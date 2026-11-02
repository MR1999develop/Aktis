//! A small read-only TIFF reader, just enough to find tags of a DNG file
//! that the decoder does not use (the opcode lists) and the Exif data of
//! JPEG and TIFF pictures. It reads the IFD tables and the values asked
//! for, never the image data, and gives up quietly (`None`) on anything
//! malformed.

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};

/// Tags read by this crate.
pub(crate) mod tag {
    /// NewSubFileType: 0 for the main image, bit 0 set for reduced-size previews.
    pub(crate) const NEW_SUBFILE_TYPE: u16 = 254;
    /// PhotometricInterpretation.
    pub(crate) const PHOTOMETRIC: u16 = 262;
    /// SubIFDs: offsets of child IFDs (DNG keeps the raw image in one).
    pub(crate) const SUB_IFDS: u16 = 330;
    /// DNGVersion, present in the first IFD of every DNG file.
    pub(crate) const DNG_VERSION: u16 = 50706;
    /// OpcodeList1: applied to the raw data as stored.
    pub(crate) const OPCODE_LIST1: u16 = 51008;
    /// OpcodeList2: applied after black/white scaling, before demosaicing.
    pub(crate) const OPCODE_LIST2: u16 = 51009;
    /// OpcodeList3: applied after demosaicing.
    pub(crate) const OPCODE_LIST3: u16 = 51022;
}

/// PhotometricInterpretation of a colour filter array mosaic.
pub(crate) const PHOTOMETRIC_CFA: u32 = 32803;
/// PhotometricInterpretation of demosaiced linear data (Linear DNG, ProRAW).
pub(crate) const PHOTOMETRIC_LINEAR_RAW: u32 = 34892;

/// Most IFDs followed, against loops and hostile files (real DNGs have a handful).
const MAX_IFDS: usize = 64;
/// Deepest SubIFD nesting followed.
const MAX_DEPTH: usize = 4;
/// Largest value read (opcode lists of real files are a few kilobytes).
const MAX_VALUE_BYTES: u64 = 64 << 20;

/// One IFD entry: tag, field type, count and the 4-byte value or offset field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    field: [u8; 4],
}

/// One image file directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Ifd {
    entries: Vec<Entry>,
}

impl Ifd {
    /// The entry for `tag`, if present.
    pub(crate) fn get(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|e| e.tag == tag)
    }
}

/// A TIFF file opened for reading tags.
#[derive(Debug)]
pub(crate) struct TiffReader<R> {
    reader: R,
    big_endian: bool,
    len: u64,
    first_ifd: u32,
}

impl<R: Read + Seek> TiffReader<R> {
    /// Reads the header; `None` when the data is not a (classic) TIFF file.
    pub(crate) fn new(mut reader: R) -> Option<Self> {
        let len = reader.seek(SeekFrom::End(0)).ok()?;
        let mut header = [0_u8; 8];
        reader.seek(SeekFrom::Start(0)).ok()?;
        reader.read_exact(&mut header).ok()?;
        let big_endian = match &header[..4] {
            b"II*\0" => false,
            b"MM\0*" => true,
            _ => return None,
        };
        let mut tiff = Self { reader, big_endian, len, first_ifd: 0 };
        tiff.first_ifd = tiff.u32([header[4], header[5], header[6], header[7]]);
        Some(tiff)
    }

    fn u16(&self, b: [u8; 2]) -> u16 {
        if self.big_endian { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) }
    }

    fn u32(&self, b: [u8; 4]) -> u32 {
        if self.big_endian { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) }
    }

    /// `len` bytes at `offset`, or `None` when they are not all in the file.
    fn read_at(&mut self, offset: u64, len: u64) -> Option<Vec<u8>> {
        if len > MAX_VALUE_BYTES || offset.checked_add(len)? > self.len {
            return None;
        }
        let mut buf = vec![0_u8; usize::try_from(len).ok()?];
        self.reader.seek(SeekFrom::Start(offset)).ok()?;
        self.reader.read_exact(&mut buf).ok()?;
        Some(buf)
    }

    /// The IFD at `offset` and the offset of the next one in its chain (0 = none).
    fn read_ifd(&mut self, offset: u32) -> Option<(Ifd, u32)> {
        let count = self.read_at(u64::from(offset), 2)?;
        let count = self.u16([count[0], count[1]]);
        let table = self.read_at(u64::from(offset) + 2, u64::from(count) * 12 + 4)?;
        let entries = table
            .as_chunks::<12>()
            .0
            .iter()
            .map(|e| Entry {
                tag: self.u16([e[0], e[1]]),
                kind: self.u16([e[2], e[3]]),
                count: self.u32([e[4], e[5], e[6], e[7]]),
                field: [e[8], e[9], e[10], e[11]],
            })
            .collect();
        let n = table.len() - 4;
        let next = self.u32([table[n], table[n + 1], table[n + 2], table[n + 3]]);
        Some((Ifd { entries }, next))
    }

    /// The first IFD of the main chain (IFD0: camera, orientation, the
    /// pointer to the Exif IFD).
    pub(crate) fn first(&mut self) -> Option<Ifd> {
        self.ifd_at(self.first_ifd)
    }

    /// The IFD at `offset` (an Exif or GPS IFD pointer), without its chain.
    pub(crate) fn ifd_at(&mut self, offset: u32) -> Option<Ifd> {
        if offset == 0 {
            return None;
        }
        self.read_ifd(offset).map(|(ifd, _)| ifd)
    }

    /// Every IFD of the file: the main chain, each followed by its SubIFDs
    /// (depth first), in file order. Damaged parts are skipped.
    pub(crate) fn ifds(&mut self) -> Vec<Ifd> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        self.walk(self.first_ifd, 0, &mut out, &mut seen);
        out
    }

    fn walk(&mut self, mut offset: u32, depth: usize, out: &mut Vec<Ifd>, seen: &mut HashSet<u32>) {
        while offset != 0 && out.len() < MAX_IFDS && seen.insert(offset) {
            let Some((ifd, next)) = self.read_ifd(offset) else { return };
            let children = if depth < MAX_DEPTH {
                ifd.get(tag::SUB_IFDS).copied().and_then(|e| self.unsigned(&e)).unwrap_or_default()
            } else {
                Vec::new()
            };
            out.push(ifd);
            for child in children {
                self.walk(child, depth + 1, out, seen);
            }
            offset = next;
        }
    }

    /// The bytes of an entry's value, as stored (inline or at its offset).
    pub(crate) fn value_bytes(&mut self, entry: &Entry) -> Option<Vec<u8>> {
        let size = match entry.kind {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 => 8,
            _ => return None,
        };
        let len = u64::from(entry.count) * size;
        if len <= 4 {
            return Some(entry.field[..usize::try_from(len).ok()?].to_vec());
        }
        self.read_at(u64::from(self.u32(entry.field)), len)
    }

    /// The values of a BYTE, SHORT, LONG or IFD entry; `None` for other types.
    pub(crate) fn unsigned(&mut self, entry: &Entry) -> Option<Vec<u32>> {
        let bytes = self.value_bytes(entry)?;
        match entry.kind {
            1 => Some(bytes.iter().map(|b| u32::from(*b)).collect()),
            3 => Some(bytes.as_chunks::<2>().0.iter().map(|b| u32::from(self.u16([b[0], b[1]]))).collect()),
            4 | 13 => Some(bytes.as_chunks::<4>().0.iter().map(|b| self.u32([b[0], b[1], b[2], b[3]])).collect()),
            _ => None,
        }
    }

    /// The text of an ASCII entry, up to its first NUL, trimmed; `None` when
    /// empty or of another type.
    pub(crate) fn ascii(&mut self, ifd: &Ifd, tag: u16) -> Option<String> {
        let entry = *ifd.get(tag)?;
        if entry.kind != 2 {
            return None;
        }
        let bytes = self.value_bytes(&entry)?;
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        let text = String::from_utf8_lossy(&bytes[..end]).trim().to_owned();
        (!text.is_empty()).then_some(text)
    }

    /// The first value of a RATIONAL entry as numerator and denominator.
    pub(crate) fn rational(&mut self, ifd: &Ifd, tag: u16) -> Option<(u32, u32)> {
        let entry = *ifd.get(tag)?;
        if entry.kind != 5 {
            return None;
        }
        let b = self.value_bytes(&entry)?;
        let b = b.get(..8)?;
        Some((self.u32([b[0], b[1], b[2], b[3]]), self.u32([b[4], b[5], b[6], b[7]])))
    }

    /// The first value of a RATIONAL or SRATIONAL entry; `None` when the
    /// denominator is 0.
    pub(crate) fn real(&mut self, ifd: &Ifd, tag: u16) -> Option<f64> {
        let entry = *ifd.get(tag)?;
        let b = self.value_bytes(&entry)?;
        let b = b.get(..8)?;
        let (n, d) = (self.u32([b[0], b[1], b[2], b[3]]), self.u32([b[4], b[5], b[6], b[7]]));
        let (n, d) = match entry.kind {
            5 => (f64::from(n), f64::from(d)),
            // The bits of the signed values, as stored.
            10 => (f64::from(n.cast_signed()), f64::from(d.cast_signed())),
            _ => return None,
        };
        (d != 0.0).then(|| n / d).filter(|v| v.is_finite())
    }

    /// The first value of a BYTE, SHORT, LONG or IFD entry.
    pub(crate) fn first_unsigned(&mut self, ifd: &Ifd, tag: u16) -> Option<u32> {
        let entry = *ifd.get(tag)?;
        self.unsigned(&entry)?.first().copied()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use std::io::Cursor;

    /// A value for [`build`]: field type and its bytes in the file's byte order.
    pub(crate) enum Value {
        Short(Vec<u16>),
        Long(Vec<u32>),
        Undefined(Vec<u8>),
        Ascii(String),
        Rational(Vec<(u32, u32)>),
        SRational(Vec<(i32, i32)>),
    }

    /// One IFD for [`build`]: entries sorted by tag, and the indices (in the
    /// list given to `build`) of its SubIFDs.
    pub(crate) struct TestIfd {
        pub(crate) entries: Vec<(u16, Value)>,
        pub(crate) sub_ifds: Vec<usize>,
    }

    /// A TIFF file with `ifds`; the first one starts the main chain and the
    /// others are reached only as SubIFDs.
    #[allow(clippy::cast_possible_truncation)] // test files are tiny
    pub(crate) fn build(big_endian: bool, ifds: &[TestIfd]) -> Vec<u8> {
        let put16 = |v: u16| if big_endian { v.to_be_bytes() } else { v.to_le_bytes() };
        let put32 = |v: u32| if big_endian { v.to_be_bytes() } else { v.to_le_bytes() };
        // Layout: header, then each IFD table, then all out-of-line values.
        let table_len = |ifd: &TestIfd| 2 + 12 * (ifd.entries.len() + usize::from(!ifd.sub_ifds.is_empty())) + 4;
        let mut offsets = Vec::new();
        let mut at = 8;
        for ifd in ifds {
            offsets.push(at);
            at += table_len(ifd);
        }
        let mut out = Vec::new();
        out.extend_from_slice(if big_endian { b"MM\0*" } else { b"II*\0" });
        out.extend_from_slice(&put32(8));
        let mut data = Vec::new();
        for ifd in ifds {
            let mut entries: Vec<(u16, u16, u32, Vec<u8>)> = ifd
                .entries
                .iter()
                .map(|(t, v)| match v {
                    Value::Short(s) => (*t, 3, s.len() as u32, s.iter().flat_map(|x| put16(*x)).collect()),
                    Value::Long(l) => (*t, 4, l.len() as u32, l.iter().flat_map(|x| put32(*x)).collect()),
                    Value::Undefined(b) => (*t, 7, b.len() as u32, b.clone()),
                    Value::Ascii(text) => {
                        let mut b = text.as_bytes().to_vec();
                        b.push(0);
                        (*t, 2, b.len() as u32, b)
                    }
                    Value::Rational(r) => {
                        (*t, 5, r.len() as u32, r.iter().flat_map(|(n, d)| [put32(*n), put32(*d)]).flatten().collect())
                    }
                    Value::SRational(r) => (
                        *t,
                        10,
                        r.len() as u32,
                        r.iter()
                            .flat_map(|(n, d)| [put32(n.cast_unsigned()), put32(d.cast_unsigned())])
                            .flatten()
                            .collect(),
                    ),
                })
                .collect();
            if !ifd.sub_ifds.is_empty() {
                let subs = ifd.sub_ifds.iter().map(|i| offsets[*i] as u32);
                entries.push((tag::SUB_IFDS, 13, ifd.sub_ifds.len() as u32, subs.flat_map(put32).collect()));
            }
            entries.sort_by_key(|e| e.0);
            out.extend_from_slice(&put16(entries.len() as u16));
            for (t, kind, count, bytes) in entries {
                out.extend_from_slice(&put16(t));
                out.extend_from_slice(&put16(kind));
                out.extend_from_slice(&put32(count));
                if bytes.len() <= 4 {
                    let mut field = bytes.clone();
                    field.resize(4, 0);
                    out.extend_from_slice(&field);
                } else {
                    let offset = at + data.len();
                    out.extend_from_slice(&put32(offset as u32));
                    data.extend_from_slice(&bytes);
                }
            }
            out.extend_from_slice(&put32(0));
        }
        out.extend_from_slice(&data);
        out
    }

    #[test]
    fn both_byte_orders_and_sub_ifds_are_read() {
        for big_endian in [false, true] {
            let file = build(
                big_endian,
                &[
                    TestIfd {
                        entries: vec![
                            (tag::NEW_SUBFILE_TYPE, Value::Long(vec![1])),
                            (500, Value::Short(vec![7, 8, 9])),
                        ],
                        sub_ifds: vec![1],
                    },
                    TestIfd {
                        entries: vec![
                            (tag::NEW_SUBFILE_TYPE, Value::Long(vec![0])),
                            (600, Value::Undefined(vec![1; 9])),
                        ],
                        sub_ifds: vec![],
                    },
                ],
            );
            let mut tiff = TiffReader::new(Cursor::new(file)).unwrap();
            let ifds = tiff.ifds();
            assert_eq!(ifds.len(), 2);
            assert_eq!(tiff.first_unsigned(&ifds[0], tag::NEW_SUBFILE_TYPE), Some(1));
            assert_eq!(tiff.first_unsigned(&ifds[1], tag::NEW_SUBFILE_TYPE), Some(0));
            let shorts = *ifds[0].get(500).unwrap();
            assert_eq!(tiff.unsigned(&shorts), Some(vec![7, 8, 9]));
            let bytes = *ifds[1].get(600).unwrap();
            assert_eq!(tiff.value_bytes(&bytes), Some(vec![1; 9]));
        }
    }

    #[test]
    fn non_tiff_truncated_and_looping_files_do_not_panic() {
        assert!(TiffReader::new(Cursor::new(b"not a tiff file".to_vec())).is_none());
        assert!(TiffReader::new(Cursor::new(Vec::new())).is_none());

        let file = build(false, &[TestIfd { entries: vec![(600, Value::Undefined(vec![2; 40]))], sub_ifds: vec![] }]);
        for cut in 0..file.len() {
            if let Some(mut tiff) = TiffReader::new(Cursor::new(file[..cut].to_vec())) {
                for ifd in tiff.ifds() {
                    let e = *ifd.get(600).unwrap();
                    assert!(tiff.value_bytes(&e).is_none(), "cut at {cut}");
                }
            }
        }

        // An IFD whose "next" points back at itself.
        let mut looping = build(false, &[TestIfd { entries: vec![(500, Value::Short(vec![1]))], sub_ifds: vec![] }]);
        let next = 8 + 2 + 12;
        looping[next..next + 4].copy_from_slice(&8_u32.to_le_bytes());
        let mut tiff = TiffReader::new(Cursor::new(looping)).unwrap();
        assert_eq!(tiff.ifds().len(), 1);
    }
}
