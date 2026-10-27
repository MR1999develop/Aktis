//! Binary PPM (P6, 8 bits) input: lets `rne mask` be tried on any photo
//! converted with `jpegtopnm` or `convert`, without a RAW file.

use core_model::{ImageF32, Rgba, WorkingSpace};

/// Decodes a P6 file into a linear Rec.2020 image (the editor's working space).
pub fn decode(bytes: &[u8]) -> Result<ImageF32, String> {
    let mut pos = 0;
    let mut token = || -> Result<String, String> {
        loop {
            // Skip white space and comments.
            while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }
            if bytes.get(pos) == Some(&b'#') {
                while pos < bytes.len() && bytes[pos] != b'\n' {
                    pos += 1;
                }
                continue;
            }
            let start = pos;
            while pos < bytes.len() && !bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }
            if start == pos {
                return Err("truncated PPM header".to_owned());
            }
            return Ok(String::from_utf8_lossy(&bytes[start..pos]).into_owned());
        }
    };
    if token()? != "P6" {
        return Err("only binary PPM files (P6) are supported".to_owned());
    }
    let number = |t: String| t.parse::<u32>().map_err(|_| format!("bad number '{t}' in PPM header"));
    let width = number(token()?)?;
    let height = number(token()?)?;
    let max = number(token()?)?;
    if max == 0 || max > 255 {
        return Err("only 8-bit PPM files are supported".to_owned());
    }
    // Exactly one white-space byte separates the header from the pixels.
    let data = bytes.get(pos + 1..).ok_or("PPM has no pixel data")?;
    let n = width as usize * height as usize;
    if data.len() < n * 3 {
        return Err(format!("PPM pixel data is truncated: {} of {} bytes", data.len(), n * 3));
    }
    let to_rec2020 = core_color::rgb_to_rgb(WorkingSpace::LinearSrgb, WorkingSpace::LinearRec2020);
    let scale = 1.0 / max as f32;
    let pixels: Vec<Rgba> = data[..n * 3]
        .chunks_exact(3)
        .map(|c| {
            let lin = [c[0], c[1], c[2]].map(|v| core_color::srgb_decode(f32::from(v) * scale));
            let [r, g, b] = to_rec2020.apply(lin);
            [r, g, b, 1.0]
        })
        .collect();
    ImageF32::from_pixels(width, height, WorkingSpace::LinearRec2020, pixels).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_ppm_decodes_to_linear_light() {
        let mut f = b"P6\n# a comment\n2 1\n255\n".to_vec();
        f.extend_from_slice(&[255, 255, 255, 118, 118, 118]);
        let img = decode(&f).expect("decodes");
        assert_eq!((img.width(), img.height()), (2, 1));
        let white = img.get(0, 0).expect("pixel");
        let grey = img.get(1, 0).expect("pixel");
        assert!(white.iter().take(3).all(|v| (v - 1.0).abs() < 1e-4), "{white:?}");
        assert!(grey.iter().take(3).all(|v| (v - 0.181).abs() < 2e-3), "{grey:?}");
    }

    #[test]
    fn other_formats_and_truncated_files_are_refused() {
        assert!(decode(b"P3\n1 1\n255\n0 0 0").is_err());
        assert!(decode(b"P6\n2 2\n255\n\x00\x00\x00").is_err());
        assert!(decode(b"P6\n1 1\n65535\n\x00\x00\x00\x00\x00\x00").is_err());
        assert!(decode(b"P6\n1").is_err());
    }
}
