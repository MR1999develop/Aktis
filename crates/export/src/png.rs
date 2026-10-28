//! PNG files: 8 or 16 bits per channel, RGB, with the sRGB profile (iCCP),
//! EXIF (eXIf) and XMP (iTXt) chunks.

use crate::deflate::zlib_compress;
use crate::{BitDepth, ExportError, RgbImage16};

/// Bytes per IDAT chunk.
const IDAT_BYTES: usize = 1 << 18;

/// The CRC-32 of PNG chunks.
fn crc32(parts: &[&[u8]]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0_u32; 256];
        for (n, entry) in (0_u32..).zip(t.iter_mut()) {
            let mut c = n;
            for _ in 0..8 {
                c = if c & 1 == 1 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *entry = c;
        }
        t
    });
    let mut c = 0xFFFF_FFFF_u32;
    for part in parts {
        for &b in *part {
            c = table[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
        }
    }
    c ^ 0xFFFF_FFFF
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) -> Result<(), ExportError> {
    let len = u32::try_from(data.len()).map_err(|_| ExportError::FileTooLarge)?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
    Ok(())
}

/// The Paeth predictor of the PNG specification.
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (i16::from(a), i16::from(b), i16::from(c));
    let p = ia + ib - ic;
    let (pa, pb, pc) = ((p - ia).abs(), (p - ib).abs(), (p - ic).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Filters `row` against `prev` with each of the five filters and appends
/// the one whose bytes, read as signed values, sum smallest (the usual
/// heuristic): filter type byte, then the filtered row.
fn filter_row(row: &[u8], prev: &[u8], bpp: usize, out: &mut Vec<u8>, scratch: &mut [Vec<u8>; 5]) {
    for (kind, buf) in scratch.iter_mut().enumerate() {
        buf.clear();
        for i in 0..row.len() {
            let a = if i >= bpp { row[i - bpp] } else { 0 };
            let b = prev[i];
            let c = if i >= bpp { prev[i - bpp] } else { 0 };
            let predicted = match kind {
                0 => 0,
                1 => a,
                2 => b,
                #[allow(clippy::cast_possible_truncation)] // the average of two bytes
                3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                _ => paeth(a, b, c),
            };
            buf.push(row[i].wrapping_sub(predicted));
        }
    }
    let cost = |buf: &Vec<u8>| buf.iter().map(|&v| u64::from(v.min(0_u8.wrapping_sub(v)))).sum::<u64>();
    let (best, buf) = scratch.iter().enumerate().min_by_key(|(_, b)| cost(b)).unwrap_or((0, &scratch[0]));
    #[allow(clippy::cast_possible_truncation)] // five filters
    out.push(best as u8);
    out.extend_from_slice(buf);
}

/// Encodes `image` as a PNG.
///
/// # Errors
/// [`ExportError::FileTooLarge`] if a chunk would exceed the PNG limit.
pub(crate) fn encode_png(
    image: &RgbImage16,
    depth: BitDepth,
    icc: &[u8],
    exif: Option<&[u8]>,
    xmp: Option<&str>,
) -> Result<Vec<u8>, ExportError> {
    let (w, h) = (image.width as usize, image.height as usize);
    let bpp = match depth {
        BitDepth::Eight => 3,
        BitDepth::Sixteen => 6,
    };
    let row_bytes = w * bpp;
    let mut filtered = Vec::with_capacity((row_bytes + 1) * h);
    let mut prev = vec![0_u8; row_bytes];
    let mut row = Vec::with_capacity(row_bytes);
    let mut scratch: [Vec<u8>; 5] = Default::default();
    for y in 0..h {
        row.clear();
        let samples = &image.pixels[y * w * 3..(y + 1) * w * 3];
        match depth {
            BitDepth::Eight => row.extend(samples.iter().map(|&v| crate::to_u8(v))),
            BitDepth::Sixteen => row.extend(samples.iter().flat_map(|v| v.to_be_bytes())),
        }
        filter_row(&row, &prev, bpp, &mut filtered, &mut scratch);
        std::mem::swap(&mut prev, &mut row);
    }
    let compressed = zlib_compress(&filtered);
    drop(filtered);

    let mut out = Vec::with_capacity(compressed.len() + icc.len() + 1024);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&image.width.to_be_bytes());
    ihdr.extend_from_slice(&image.height.to_be_bytes());
    ihdr.push(if bpp == 3 { 8 } else { 16 });
    ihdr.extend_from_slice(&[2, 0, 0, 0]); // RGB, deflate, adaptive filters, no interlace
    chunk(&mut out, b"IHDR", &ihdr)?;
    let mut iccp = b"sRGB\0\0".to_vec(); // profile name, compression method 0
    iccp.extend(zlib_compress(icc));
    chunk(&mut out, b"iCCP", &iccp)?;
    if let Some(exif) = exif {
        chunk(&mut out, b"eXIf", exif)?;
    }
    if let Some(xmp) = xmp {
        // Keyword, no compression, no language, no translated keyword.
        let mut itxt = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
        itxt.extend_from_slice(xmp.as_bytes());
        chunk(&mut out, b"iTXt", &itxt)?;
    }
    for part in compressed.chunks(IDAT_BYTES) {
        chunk(&mut out, b"IDAT", part)?;
    }
    chunk(&mut out, b"IEND", &[])?;
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::deflate::inflate::zlib_decompress;

    /// The chunks of a PNG file: (type, data), CRCs checked.
    pub(crate) fn chunks(png: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let mut out = Vec::new();
        let mut p = 8;
        while p < png.len() {
            let len = u32::from_be_bytes(png[p..p + 4].try_into().expect("4 bytes")) as usize;
            let kind: [u8; 4] = png[p + 4..p + 8].try_into().expect("4 bytes");
            let data = png[p + 8..p + 8 + len].to_vec();
            let crc = u32::from_be_bytes(png[p + 8 + len..p + 12 + len].try_into().expect("4 bytes"));
            assert_eq!(crc, crc32(&[&kind, &data]), "CRC of {}", String::from_utf8_lossy(&kind));
            out.push((kind, data));
            p += 12 + len;
        }
        out
    }

    /// Undoes the PNG filters: the raw rows.
    fn unfilter(data: &[u8], row_bytes: usize, bpp: usize) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        for (y, line) in data.chunks(row_bytes + 1).enumerate() {
            let kind = line[0];
            for i in 0..row_bytes {
                let a = if i >= bpp { out[y * row_bytes + i - bpp] } else { 0 };
                let b = if y > 0 { out[(y - 1) * row_bytes + i] } else { 0 };
                let c = if y > 0 && i >= bpp { out[(y - 1) * row_bytes + i - bpp] } else { 0 };
                let p = match kind {
                    0 => 0,
                    1 => a,
                    2 => b,
                    3 => u8::try_from((u16::from(a) + u16::from(b)) / 2).expect("byte"),
                    _ => paeth(a, b, c),
                };
                out.push(line[1 + i].wrapping_add(p));
            }
        }
        out
    }

    fn gradient(w: u32, h: u32) -> RgbImage16 {
        let mut pixels = Vec::new();
        for y in 0..h {
            for x in 0..w {
                pixels.extend(
                    [x * 900 % 65_536, y * 1300 % 65_536, (x * y * 37) % 65_536]
                        .map(|v| u16::try_from(v).expect("16 bits")),
                );
            }
        }
        RgbImage16 { width: w, height: h, pixels }
    }

    #[test]
    fn a_crc_matches_the_known_value() {
        assert_eq!(crc32(&[b"IEND"]), 0xAE42_6082);
    }

    #[test]
    fn png_pixels_and_chunks_survive_a_round_trip() {
        let img = gradient(37, 11);
        for (depth, bpp) in [(BitDepth::Sixteen, 6), (BitDepth::Eight, 3)] {
            let png = encode_png(&img, depth, b"profile", Some(b"II*\0exif"), Some("<x/>")).expect("encodes");
            let chunks = chunks(&png);
            let kinds: Vec<&[u8]> = chunks.iter().map(|(k, _)| &k[..]).collect();
            assert_eq!(kinds, [b"IHDR", b"iCCP", b"eXIf", b"iTXt", b"IDAT", b"IEND"]);
            let ihdr = &chunks[0].1;
            assert_eq!(&ihdr[..8], &[0, 0, 0, 37, 0, 0, 0, 11]);
            assert_eq!(ihdr[8], if bpp == 6 { 16 } else { 8 });
            assert_eq!(zlib_decompress(&chunks[1].1[6..]).expect("profile"), b"profile");
            assert_eq!(chunks[2].1, b"II*\0exif");
            assert!(chunks[3].1.starts_with(b"XML:com.adobe.xmp\0"));
            let idat: Vec<u8> = chunks.iter().filter(|(k, _)| k == b"IDAT").flat_map(|(_, d)| d.clone()).collect();
            let rows = unfilter(&zlib_decompress(&idat).expect("pixels"), 37 * bpp, bpp);
            let expected: Vec<u8> = match depth {
                BitDepth::Sixteen => img.pixels.iter().flat_map(|v| v.to_be_bytes()).collect(),
                BitDepth::Eight => img.pixels.iter().map(|&v| crate::to_u8(v)).collect(),
            };
            assert_eq!(rows, expected);
        }
    }
}
