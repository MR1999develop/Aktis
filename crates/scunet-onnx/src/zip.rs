//! Just enough of the zip format to read a PyTorch checkpoint.
//!
//! `torch.save` writes an uncompressed ("stored") zip, so the entries are
//! read in place from the input bytes, without copies. Compressed or
//! encrypted entries are refused instead of being misread. Zip64 records
//! are understood because `torch.save` always writes them, and very large
//! checkpoints need them.

use crate::ConvertError;

const EOCD_SIG: u32 = 0x0605_4b50;
const ZIP64_LOCATOR_SIG: u32 = 0x0706_4b50;
const ZIP64_EOCD_SIG: u32 = 0x0606_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;
const EOCD_LEN: usize = 22;
const ZIP64_LOCATOR_LEN: usize = 20;
/// Compression method 0: the entry's bytes are the file's bytes.
const METHOD_STORED: u16 = 0;
/// General-purpose flag bit 0: the entry is encrypted.
const FLAG_ENCRYPTED: u16 = 1;

fn err(msg: impl Into<String>) -> ConvertError {
    ConvertError::Archive(msg.into())
}

/// Little-endian reader over a byte slice that turns every out-of-range
/// read into an error, so a truncated or hostile file cannot panic.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn at(buf: &'a [u8], pos: usize) -> Self {
        Self { buf, pos }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ConvertError> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.buf.len()).ok_or_else(|| err("truncated zip file"))?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ConvertError> {
        let mut a = [0; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    fn u16(&mut self) -> Result<u16, ConvertError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, ConvertError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, ConvertError> {
        Ok(u64::from_le_bytes(self.array()?))
    }
}

fn to_usize(v: u64) -> Result<usize, ConvertError> {
    usize::try_from(v).map_err(|_| err("zip offset beyond the addressable memory"))
}

/// One file of the archive, as described by the central directory.
#[derive(Debug, Clone)]
struct Entry {
    name: String,
    method: u16,
    flags: u16,
    crc32: u32,
    compressed_size: u64,
    size: u64,
    local_header: u64,
}

/// A zip archive held in memory; entries borrow from its bytes.
#[derive(Debug)]
pub(crate) struct Archive<'a> {
    bytes: &'a [u8],
    entries: Vec<Entry>,
}

impl<'a> Archive<'a> {
    /// Reads the central directory of `bytes`.
    pub(crate) fn new(bytes: &'a [u8]) -> Result<Self, ConvertError> {
        let eocd = find_eocd(bytes)?;
        let mut r = Reader::at(bytes, eocd + 4);
        let disk = r.u16()?;
        let cd_disk = r.u16()?;
        let _entries_on_disk = r.u16()?;
        let mut count = u64::from(r.u16()?);
        let mut cd_size = u64::from(r.u32()?);
        let mut cd_offset = u64::from(r.u32()?);
        if disk != 0 || cd_disk != 0 {
            return Err(err("multi-part zip files are not supported"));
        }
        // Saturated fields mean the real values are in the zip64 record.
        if count == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_offset == 0xFFFF_FFFF {
            (count, cd_size, cd_offset) = read_zip64_eocd(bytes, eocd)?;
        }
        let start = to_usize(cd_offset)?;
        let end = to_usize(cd_offset.checked_add(cd_size).ok_or_else(|| err("bad central directory"))?)?;
        if end > bytes.len() {
            return Err(err("the central directory lies outside the file"));
        }
        let mut r = Reader::at(&bytes[..end], start);
        // Each central record is at least 46 bytes: bound the reservation
        // by what the directory can hold, not by the claimed count.
        let mut entries = Vec::with_capacity(to_usize(count)?.min(cd_size_bound(cd_size)));
        for _ in 0..count {
            entries.push(read_central_entry(&mut r)?);
        }
        Ok(Self { bytes, entries })
    }

    /// Names of the entries, in central-directory order.
    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name.as_str())
    }

    /// The bytes of the entry `name`, after checking its CRC-32 like
    /// Python's `zipfile` does.
    pub(crate) fn read(&self, name: &str) -> Result<&'a [u8], ConvertError> {
        let e = self.entries.iter().find(|e| e.name == name).ok_or_else(|| err(format!("{name} is missing")))?;
        if e.flags & FLAG_ENCRYPTED != 0 {
            return Err(err(format!("{name} is encrypted")));
        }
        if e.method != METHOD_STORED {
            return Err(err(format!("{name} is compressed (method {}); only stored entries are supported", e.method)));
        }
        if e.compressed_size != e.size {
            return Err(err(format!("{name}: stored entry with differing sizes")));
        }
        let mut r = Reader::at(self.bytes, to_usize(e.local_header)?);
        if r.u32()? != LOCAL_SIG {
            return Err(err(format!("{name}: bad local header")));
        }
        // The local header repeats the central one; only the lengths of
        // its variable parts are needed (torch pads the extra field so the
        // data is aligned).
        r.take(22)?;
        let name_len = usize::from(r.u16()?);
        let extra_len = usize::from(r.u16()?);
        r.take(name_len + extra_len)?;
        let data = r.take(to_usize(e.size)?)?;
        if crc32(data) != e.crc32 {
            return Err(err(format!("{name}: CRC-32 mismatch, the file is damaged")));
        }
        Ok(data)
    }
}

fn cd_size_bound(cd_size: u64) -> usize {
    usize::try_from(cd_size / 46).unwrap_or(usize::MAX)
}

/// Position of the end-of-central-directory record: the last one in the
/// final 64 KiB + 22 bytes (it may be followed by a comment).
fn find_eocd(bytes: &[u8]) -> Result<usize, ConvertError> {
    if bytes.len() < EOCD_LEN {
        return Err(err("not a zip file"));
    }
    let last = bytes.len() - EOCD_LEN;
    let first = last.saturating_sub(0xFFFF);
    (first..=last)
        .rev()
        .find(|&p| bytes[p..p + 4] == EOCD_SIG.to_le_bytes())
        .ok_or_else(|| err("not a zip file (no end of central directory)"))
}

/// (entry count, central directory size, central directory offset) from
/// the zip64 end record, found through the locator just before `eocd`.
fn read_zip64_eocd(bytes: &[u8], eocd: usize) -> Result<(u64, u64, u64), ConvertError> {
    let loc = eocd.checked_sub(ZIP64_LOCATOR_LEN).ok_or_else(|| err("missing zip64 locator"))?;
    let mut r = Reader::at(bytes, loc);
    if r.u32()? != ZIP64_LOCATOR_SIG {
        return Err(err("missing zip64 locator"));
    }
    let _disk = r.u32()?;
    let record = to_usize(r.u64()?)?;
    let mut r = Reader::at(bytes, record);
    if r.u32()? != ZIP64_EOCD_SIG {
        return Err(err("bad zip64 end of central directory"));
    }
    let _record_size = r.u64()?;
    let _versions = r.u32()?;
    let disk = r.u32()?;
    let cd_disk = r.u32()?;
    let _entries_on_disk = r.u64()?;
    let count = r.u64()?;
    let cd_size = r.u64()?;
    let cd_offset = r.u64()?;
    if disk != 0 || cd_disk != 0 {
        return Err(err("multi-part zip files are not supported"));
    }
    Ok((count, cd_size, cd_offset))
}

fn read_central_entry(r: &mut Reader<'_>) -> Result<Entry, ConvertError> {
    if r.u32()? != CENTRAL_SIG {
        return Err(err("bad central directory entry"));
    }
    let _made_by = r.u16()?;
    let _needed = r.u16()?;
    let flags = r.u16()?;
    let method = r.u16()?;
    let _time = r.u32()?;
    let crc32 = r.u32()?;
    let mut compressed_size = u64::from(r.u32()?);
    let mut size = u64::from(r.u32()?);
    let name_len = usize::from(r.u16()?);
    let extra_len = usize::from(r.u16()?);
    let comment_len = usize::from(r.u16()?);
    let _disk = r.u16()?;
    let _internal = r.u16()?;
    let _external = r.u32()?;
    let mut local_header = u64::from(r.u32()?);
    let name = String::from_utf8_lossy(r.take(name_len)?).into_owned();
    let mut extra = Reader::at(r.take(extra_len)?, 0);
    r.take(comment_len)?;
    // Zip64 extra field: 64-bit values for exactly the fields that are
    // saturated above, in this order.
    while extra.pos + 4 <= extra.buf.len() {
        let id = extra.u16()?;
        let len = usize::from(extra.u16()?);
        let mut field = Reader::at(extra.take(len)?, 0);
        if id == 0x0001 {
            if size == 0xFFFF_FFFF {
                size = field.u64()?;
            }
            if compressed_size == 0xFFFF_FFFF {
                compressed_size = field.u64()?;
            }
            if local_header == 0xFFFF_FFFF {
                local_header = field.u64()?;
            }
        }
    }
    Ok(Entry { name, method, flags, crc32, compressed_size, size, local_header })
}

/// CRC-32 (IEEE, reflected, as in zip), table-driven.
pub(crate) fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            #[allow(clippy::cast_possible_truncation, reason = "i < 256")]
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    };
    let mut c = !0u32;
    for &b in data {
        c = TABLE[usize::from(c.to_le_bytes()[0] ^ b)] ^ (c >> 8);
    }
    !c
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use super::*;

    /// A stored zip of `files`, laid out like `torch.save` does (data
    /// descriptor flag set, sizes in the central directory). `method`
    /// overrides the compression method of every entry, for error tests.
    pub(crate) fn make_zip(files: &[(&str, &[u8])], method: u16) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let offset = u32::try_from(out.len()).unwrap();
            let crc = crc32(data);
            let size = u32::try_from(data.len()).unwrap();
            let name_len = u16::try_from(name.len()).unwrap();
            // local header with zero sizes and a padding extra field
            out.extend_from_slice(&LOCAL_SIG.to_le_bytes());
            out.extend_from_slice(&[0, 0, 8, 8]);
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 16]);
            out.extend_from_slice(&name_len.to_le_bytes());
            out.extend_from_slice(&4u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(b"FB\0\0");
            out.extend_from_slice(data);
            central.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
            central.extend_from_slice(&[0, 0, 0, 0, 8, 8]);
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0; 4]);
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&name_len.to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_offset = u32::try_from(out.len()).unwrap();
        let count = u16::try_from(files.len()).unwrap();
        out.extend_from_slice(&central);
        out.extend_from_slice(&EOCD_SIG.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&u32::try_from(central.len()).unwrap().to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&[0; 2]);
        out
    }

    #[test]
    fn crc32_matches_known_values() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn reads_stored_entries() {
        let zip = make_zip(&[("a/data.pkl", b"hello"), ("a/data/0", &[1, 2, 3, 4])], METHOD_STORED);
        let a = Archive::new(&zip).unwrap();
        assert_eq!(a.names().collect::<Vec<_>>(), ["a/data.pkl", "a/data/0"]);
        assert_eq!(a.read("a/data.pkl").unwrap(), b"hello");
        assert_eq!(a.read("a/data/0").unwrap(), [1, 2, 3, 4]);
        assert!(matches!(a.read("a/other"), Err(ConvertError::Archive(_))));
    }

    #[test]
    fn refuses_compressed_entries() {
        let zip = make_zip(&[("x", b"abc")], 8);
        let a = Archive::new(&zip).unwrap();
        let e = a.read("x").unwrap_err().to_string();
        assert!(e.contains("compressed"), "{e}");
    }

    #[test]
    fn detects_damage_and_truncation() {
        let mut zip = make_zip(&[("x", b"abcdef")], METHOD_STORED);
        let pos = zip.windows(6).position(|w| w == b"abcdef").unwrap();
        zip[pos] = b'z';
        let a = Archive::new(&zip).unwrap();
        assert!(a.read("x").unwrap_err().to_string().contains("CRC"));
        assert!(Archive::new(&zip[..zip.len() - 1]).is_err());
        assert!(Archive::new(b"not a zip at all, definitely not").is_err());
        for n in 0..zip.len() {
            // never panics, whatever the cut
            let _ = Archive::new(&zip[..n]).map(|a| a.read("x").is_ok());
        }
    }
}
