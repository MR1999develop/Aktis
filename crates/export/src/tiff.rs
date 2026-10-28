//! TIFF structures, little-endian: the EXIF block of JPEG and PNG files,
//! and whole TIFF files (8 or 16 bits per channel, RGB, uncompressed).

use crate::{BitDepth, ExportError, RgbImage16};

/// A field value with its TIFF type.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    /// BYTE (1).
    Byte(Vec<u8>),
    /// ASCII (2): written with its terminating NUL.
    Ascii(String),
    /// SHORT (3).
    Short(Vec<u16>),
    /// LONG (4).
    Long(Vec<u32>),
    /// RATIONAL (5): numerator, denominator.
    Rational(Vec<(u32, u32)>),
    /// UNDEFINED (7).
    Undefined(Vec<u8>),
    /// SRATIONAL (10).
    SRational(Vec<(i32, i32)>),
}

impl Value {
    fn kind(&self) -> u16 {
        match self {
            Self::Byte(_) => 1,
            Self::Ascii(_) => 2,
            Self::Short(_) => 3,
            Self::Long(_) => 4,
            Self::Rational(_) => 5,
            Self::Undefined(_) => 7,
            Self::SRational(_) => 10,
        }
    }

    /// Count and bytes, little-endian.
    fn encode(&self) -> (usize, Vec<u8>) {
        match self {
            Self::Byte(v) | Self::Undefined(v) => (v.len(), v.clone()),
            Self::Ascii(s) => {
                let mut b = s.as_bytes().to_vec();
                b.push(0);
                (b.len(), b)
            }
            Self::Short(v) => (v.len(), v.iter().flat_map(|x| x.to_le_bytes()).collect()),
            Self::Long(v) => (v.len(), v.iter().flat_map(|x| x.to_le_bytes()).collect()),
            Self::Rational(v) => {
                (v.len(), v.iter().flat_map(|(n, d)| n.to_le_bytes().into_iter().chain(d.to_le_bytes())).collect())
            }
            Self::SRational(v) => {
                (v.len(), v.iter().flat_map(|(n, d)| n.to_le_bytes().into_iter().chain(d.to_le_bytes())).collect())
            }
        }
    }
}

/// One entry of a directory.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Field {
    /// A tag and its value.
    Value(u16, Value),
    /// A tag pointing to another directory (EXIF, GPS).
    SubIfd(u16, Ifd),
    /// A LONG tag holding the offset of the data written after the
    /// structure (the strip of a TIFF file).
    DataOffset(u16),
}

impl Field {
    fn tag(&self) -> u16 {
        match self {
            Self::Value(t, _) | Self::SubIfd(t, _) | Self::DataOffset(t) => *t,
        }
    }
}

/// An image file directory.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Ifd {
    pub(crate) fields: Vec<Field>,
}

fn even(n: usize) -> usize {
    n + (n & 1)
}

impl Ifd {
    pub(crate) fn push(&mut self, tag: u16, value: Value) {
        self.fields.push(Field::Value(tag, value));
    }

    /// Bytes this directory takes with its values and sub-directories.
    fn size(&self) -> usize {
        let mut size = 2 + 12 * self.fields.len() + 4;
        for f in &self.fields {
            match f {
                Field::Value(_, v) => {
                    let (_, bytes) = v.encode();
                    if bytes.len() > 4 {
                        size += even(bytes.len());
                    }
                }
                Field::SubIfd(_, ifd) => size += ifd.size(),
                Field::DataOffset(_) => {}
            }
        }
        size
    }

    /// Appends this directory at `out.len()` (offsets count from `origin`,
    /// the start of the TIFF header). `data_offset` is the value of
    /// [`Field::DataOffset`] fields.
    fn write(&self, out: &mut Vec<u8>, origin: usize, data_offset: u32) -> Result<(), ExportError> {
        let offset = |at: usize| u32::try_from(at - origin).map_err(|_| ExportError::FileTooLarge);
        let mut fields: Vec<&Field> = self.fields.iter().collect();
        fields.sort_by_key(|f| f.tag());
        let start = out.len();
        let mut extra_at = start + 2 + 12 * fields.len() + 4;
        let mut extra: Vec<u8> = Vec::new();
        let mut subs: Vec<&Ifd> = Vec::new();
        let values_size: usize = fields
            .iter()
            .map(|f| match f {
                Field::Value(_, v) => {
                    let n = v.encode().1.len();
                    if n > 4 { even(n) } else { 0 }
                }
                _ => 0,
            })
            .sum();
        let mut sub_at = extra_at + values_size;
        let count = u16::try_from(fields.len()).map_err(|_| ExportError::FileTooLarge)?;
        out.extend_from_slice(&count.to_le_bytes());
        for f in &fields {
            out.extend_from_slice(&f.tag().to_le_bytes());
            match f {
                Field::Value(_, v) => {
                    let (n, bytes) = v.encode();
                    out.extend_from_slice(&v.kind().to_le_bytes());
                    out.extend_from_slice(&u32::try_from(n).map_err(|_| ExportError::FileTooLarge)?.to_le_bytes());
                    if bytes.len() <= 4 {
                        let mut inline = [0_u8; 4];
                        inline[..bytes.len()].copy_from_slice(&bytes);
                        out.extend_from_slice(&inline);
                    } else {
                        out.extend_from_slice(&offset(extra_at)?.to_le_bytes());
                        extra_at += even(bytes.len());
                        extra.extend_from_slice(&bytes);
                        if bytes.len() % 2 == 1 {
                            extra.push(0);
                        }
                    }
                }
                Field::SubIfd(_, ifd) => {
                    out.extend_from_slice(&4_u16.to_le_bytes());
                    out.extend_from_slice(&1_u32.to_le_bytes());
                    out.extend_from_slice(&offset(sub_at)?.to_le_bytes());
                    sub_at += ifd.size();
                    subs.push(ifd);
                }
                Field::DataOffset(_) => {
                    out.extend_from_slice(&4_u16.to_le_bytes());
                    out.extend_from_slice(&1_u32.to_le_bytes());
                    out.extend_from_slice(&data_offset.to_le_bytes());
                }
            }
        }
        out.extend_from_slice(&0_u32.to_le_bytes()); // no next directory
        out.extend_from_slice(&extra);
        for ifd in subs {
            ifd.write(out, origin, data_offset)?;
        }
        Ok(())
    }
}

/// A TIFF structure: header and `ifd0` (EXIF blocks have no data after it).
pub(crate) fn tiff_bytes(ifd0: &Ifd) -> Result<Vec<u8>, ExportError> {
    let mut out = Vec::with_capacity(8 + ifd0.size());
    out.extend_from_slice(b"II*\0");
    out.extend_from_slice(&8_u32.to_le_bytes());
    ifd0.write(&mut out, 0, 0)?;
    Ok(out)
}

/// Tags of a TIFF image directory.
mod tag {
    pub const WIDTH: u16 = 256;
    pub const HEIGHT: u16 = 257;
    pub const BITS: u16 = 258;
    pub const COMPRESSION: u16 = 259;
    pub const PHOTOMETRIC: u16 = 262;
    pub const STRIP_OFFSETS: u16 = 273;
    pub const SAMPLES: u16 = 277;
    pub const ROWS_PER_STRIP: u16 = 278;
    pub const STRIP_BYTES: u16 = 279;
    pub const X_RESOLUTION: u16 = 282;
    pub const Y_RESOLUTION: u16 = 283;
    pub const PLANAR: u16 = 284;
    pub const RESOLUTION_UNIT: u16 = 296;
    pub const XMP: u16 = 700;
    pub const ICC: u16 = 34_675;
}

/// A whole TIFF file: `metadata` (the EXIF fields of IFD0, sub-directories
/// included) merged with the image fields, then the pixels in one strip.
pub(crate) fn encode_tiff(
    image: &RgbImage16,
    depth: BitDepth,
    icc: &[u8],
    metadata: Ifd,
    xmp: Option<&str>,
) -> Result<Vec<u8>, ExportError> {
    let bits: u16 = match depth {
        BitDepth::Eight => 8,
        BitDepth::Sixteen => 16,
    };
    let strip_bytes = image.pixels.len() * usize::from(bits / 8);
    let strip_len = u32::try_from(strip_bytes).map_err(|_| ExportError::FileTooLarge)?;
    let mut ifd = metadata;
    // The image's own fields replace any of the same tag.
    let image_tags = [
        tag::WIDTH,
        tag::HEIGHT,
        tag::BITS,
        tag::COMPRESSION,
        tag::PHOTOMETRIC,
        tag::STRIP_OFFSETS,
        tag::SAMPLES,
        tag::ROWS_PER_STRIP,
        tag::STRIP_BYTES,
        tag::X_RESOLUTION,
        tag::Y_RESOLUTION,
        tag::PLANAR,
        tag::RESOLUTION_UNIT,
        tag::XMP,
        tag::ICC,
    ];
    ifd.fields.retain(|f| !image_tags.contains(&f.tag()));
    ifd.push(tag::WIDTH, Value::Long(vec![image.width]));
    ifd.push(tag::HEIGHT, Value::Long(vec![image.height]));
    ifd.push(tag::BITS, Value::Short(vec![bits; 3]));
    ifd.push(tag::COMPRESSION, Value::Short(vec![1]));
    ifd.push(tag::PHOTOMETRIC, Value::Short(vec![2]));
    ifd.fields.push(Field::DataOffset(tag::STRIP_OFFSETS));
    ifd.push(tag::SAMPLES, Value::Short(vec![3]));
    ifd.push(tag::ROWS_PER_STRIP, Value::Long(vec![image.height]));
    ifd.push(tag::STRIP_BYTES, Value::Long(vec![strip_len]));
    ifd.push(tag::X_RESOLUTION, Value::Rational(vec![(300, 1)]));
    ifd.push(tag::Y_RESOLUTION, Value::Rational(vec![(300, 1)]));
    ifd.push(tag::PLANAR, Value::Short(vec![1]));
    ifd.push(tag::RESOLUTION_UNIT, Value::Short(vec![2]));
    if let Some(xmp) = xmp {
        ifd.push(tag::XMP, Value::Byte(xmp.as_bytes().to_vec()));
    }
    ifd.push(tag::ICC, Value::Undefined(icc.to_vec()));

    let data_at = even(8 + ifd.size());
    let data_offset = u32::try_from(data_at).map_err(|_| ExportError::FileTooLarge)?;
    if u64::from(data_offset) + u64::from(strip_len) > u64::from(u32::MAX) {
        return Err(ExportError::FileTooLarge);
    }
    let mut out = Vec::with_capacity(data_at + strip_bytes);
    out.extend_from_slice(b"II*\0");
    out.extend_from_slice(&8_u32.to_le_bytes());
    ifd.write(&mut out, 0, data_offset)?;
    out.resize(data_at, 0);
    match depth {
        BitDepth::Eight => out.extend(image.pixels.iter().map(|&v| crate::to_u8(v))),
        BitDepth::Sixteen => out.extend(image.pixels.iter().flat_map(|v| v.to_le_bytes())),
    }
    Ok(out)
}

/// A minimal TIFF reader for the tests: directories as tag → (type, count,
/// value bytes), sub-directories followed by the caller.
#[cfg(test)]
pub(crate) mod read {
    use std::collections::BTreeMap;

    /// One directory: tag → (type, count, value bytes).
    pub(crate) type Dir = BTreeMap<u16, (u16, u32, Vec<u8>)>;

    fn u16_at(b: &[u8], p: usize) -> u16 {
        u16::from_le_bytes([b[p], b[p + 1]])
    }

    pub(crate) fn u32_at(b: &[u8], p: usize) -> u32 {
        u32::from_le_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]])
    }

    /// The directory at `offset` of the TIFF structure `b` (little-endian).
    pub(crate) fn dir(b: &[u8], offset: u32) -> Dir {
        assert_eq!(&b[..4], b"II*\0");
        let p = offset as usize;
        assert_eq!(p % 2, 0, "directories start on a word boundary");
        let n = usize::from(u16_at(b, p));
        let mut out = Dir::new();
        let mut last = 0;
        for i in 0..n {
            let e = p + 2 + 12 * i;
            let (tag, kind, count) = (u16_at(b, e), u16_at(b, e + 2), u32_at(b, e + 4));
            assert!(tag > last || i == 0, "tags sorted");
            last = tag;
            let unit = match kind {
                1 | 2 | 7 => 1,
                3 => 2,
                4 => 4,
                5 | 10 => 8,
                _ => panic!("type {kind}"),
            };
            let len = unit * count as usize;
            let bytes = if len <= 4 {
                b[e + 8..e + 8 + len].to_vec()
            } else {
                let at = u32_at(b, e + 8) as usize;
                assert_eq!(at % 2, 0, "values start on a word boundary");
                b[at..at + len].to_vec()
            };
            out.insert(tag, (kind, count, bytes));
        }
        out
    }

    /// The first directory.
    pub(crate) fn ifd0(b: &[u8]) -> Dir {
        dir(b, u32_at(b, 4))
    }

    /// The sub-directory a pointer tag of `d` leads to.
    pub(crate) fn sub(b: &[u8], d: &Dir, tag: u16) -> Dir {
        let (_, _, v) = &d[&tag];
        dir(b, u32_at(v, 0))
    }

    /// An ASCII value without its NUL.
    pub(crate) fn ascii(d: &Dir, tag: u16) -> String {
        let (kind, _, v) = &d[&tag];
        assert_eq!(*kind, 2);
        assert_eq!(v.last(), Some(&0));
        String::from_utf8(v[..v.len() - 1].to_vec()).expect("UTF-8")
    }

    /// The first SHORT or LONG value.
    pub(crate) fn number(d: &Dir, tag: u16) -> u32 {
        let (kind, _, v) = &d[&tag];
        match kind {
            3 => u32::from(u16_at(v, 0)),
            4 => u32_at(v, 0),
            _ => panic!("type {kind}"),
        }
    }

    /// RATIONAL or SRATIONAL values as numbers.
    pub(crate) fn rationals(d: &Dir, tag: u16) -> Vec<f64> {
        let (kind, _, v) = &d[&tag];
        v.chunks(8)
            .map(|c| {
                let (n, den) = (u32_at(c, 0), u32_at(c, 4));
                if *kind == 10 {
                    f64::from(n.cast_signed()) / f64::from(den.cast_signed())
                } else {
                    f64::from(n) / f64::from(den)
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::read::*;
    use super::*;

    #[test]
    fn a_directory_with_values_and_a_sub_directory_reads_back() {
        let mut exif = Ifd::default();
        exif.push(0x829A, Value::Rational(vec![(1, 250)]));
        exif.push(0x9204, Value::SRational(vec![(-2, 3)]));
        let mut ifd0 = Ifd::default();
        ifd0.push(0x0110, Value::Ascii("Model X".into()));
        ifd0.push(0x010F, Value::Ascii("Make".into()));
        ifd0.fields.push(Field::SubIfd(0x8769, exif));
        ifd0.push(0x0112, Value::Short(vec![1]));
        let b = tiff_bytes(&ifd0).expect("writes");
        let d = ifd0_of(&b);
        assert_eq!(ascii(&d, 0x010F), "Make");
        assert_eq!(ascii(&d, 0x0110), "Model X");
        assert_eq!(number(&d, 0x0112), 1);
        let e = sub(&b, &d, 0x8769);
        assert_eq!(rationals(&e, 0x829A), vec![1.0 / 250.0]);
        assert!((rationals(&e, 0x9204)[0] + 2.0 / 3.0).abs() < 1e-12);
    }

    fn ifd0_of(b: &[u8]) -> read::Dir {
        read::ifd0(b)
    }

    #[test]
    fn a_tiff_file_holds_its_pixels_and_profile() {
        let img = RgbImage16 { width: 3, height: 2, pixels: (0..18).map(|v| v * 3000).collect() };
        for (depth, bytes_per) in [(BitDepth::Sixteen, 2), (BitDepth::Eight, 1)] {
            let file = encode_tiff(&img, depth, b"icc-profile", Ifd::default(), Some("<xmp/>")).expect("encodes");
            let d = ifd0_of(&file);
            assert_eq!((number(&d, tag::WIDTH), number(&d, tag::HEIGHT)), (3, 2));
            assert_eq!(d[&tag::BITS].2, vec![bytes_per * 8, 0, bytes_per * 8, 0, bytes_per * 8, 0]);
            assert_eq!(d[&tag::ICC].2, b"icc-profile");
            assert_eq!(d[&tag::XMP].2, b"<xmp/>");
            let at = number(&d, tag::STRIP_OFFSETS) as usize;
            let len = number(&d, tag::STRIP_BYTES) as usize;
            assert_eq!(at + len, file.len());
            let strip = &file[at..];
            if bytes_per == 2 {
                assert_eq!(&strip[..6], &[0, 0, 0xB8, 0x0B, 0x70, 0x17]);
            } else {
                assert_eq!(strip.len(), 18);
                assert_eq!(strip[1], crate::to_u8(3000));
            }
        }
    }
}
