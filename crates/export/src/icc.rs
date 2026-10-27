//! A compact sRGB ICC profile (version 2.1, matrix/TRC), built in code so the
//! project ships no third-party profile file.
//!
//! Colorants are the sRGB primaries adapted to the D50 profile connection
//! space with the Bradford transform; the tone curve is the piecewise sRGB
//! curve sampled at 1024 points.

/// sRGB primaries (Bradford-adapted to D50), as in the ICC sRGB profiles.
const RED: [f64; 3] = [0.436_074_7, 0.222_504_5, 0.013_932_2];
const GREEN: [f64; 3] = [0.385_064_9, 0.716_878_6, 0.097_104_5];
const BLUE: [f64; 3] = [0.143_080_4, 0.060_616_9, 0.714_173_3];
/// The D50 white of the profile connection space.
const D50: [f64; 3] = [0.964_2, 1.0, 0.824_9];
/// Samples of the tone curve.
const CURVE_POINTS: usize = 1024;

fn s15f16(v: f64) -> [u8; 4] {
    #[allow(clippy::cast_possible_truncation)] // colorimetric values are far inside i32
    let fixed = (v * 65_536.0).round() as i32;
    fixed.to_be_bytes()
}

fn xyz_tag(v: [f64; 3]) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    for c in v {
        t.extend_from_slice(&s15f16(c));
    }
    t
}

fn text_description(s: &str) -> Vec<u8> {
    let mut t = b"desc\0\0\0\0".to_vec();
    let len = u32::try_from(s.len() + 1).unwrap_or(0);
    t.extend_from_slice(&len.to_be_bytes());
    t.extend_from_slice(s.as_bytes());
    t.push(0);
    t.extend_from_slice(&[0; 4]); // Unicode language code
    t.extend_from_slice(&[0; 4]); // Unicode count
    t.extend_from_slice(&[0; 2]); // ScriptCode code
    t.push(0); // ScriptCode count
    t.extend_from_slice(&[0; 67]); // ScriptCode string
    t
}

fn text(s: &str) -> Vec<u8> {
    let mut t = b"text\0\0\0\0".to_vec();
    t.extend_from_slice(s.as_bytes());
    t.push(0);
    t
}

fn srgb_curve() -> Vec<u8> {
    let mut t = b"curv\0\0\0\0".to_vec();
    let n = u32::try_from(CURVE_POINTS).unwrap_or(0);
    t.extend_from_slice(&n.to_be_bytes());
    for i in 0..CURVE_POINTS {
        #[allow(clippy::cast_precision_loss)] // small integers
        let x = i as f64 / (CURVE_POINTS - 1) as f64;
        let linear = if x <= 0.040_45 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0–65535
        let v = (linear.clamp(0.0, 1.0) * 65_535.0).round() as u16;
        t.extend_from_slice(&v.to_be_bytes());
    }
    t
}

/// The bytes of a complete sRGB ICC profile.
#[must_use]
pub fn srgb_profile() -> Vec<u8> {
    let curve = srgb_curve();
    // (signature, data); the three TRC tags share one data block.
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", text_description("sRGB (RAW node editor)")),
        (b"cprt", text("No copyright, use freely")),
        (b"wtpt", xyz_tag(D50)),
        (b"rXYZ", xyz_tag(RED)),
        (b"gXYZ", xyz_tag(GREEN)),
        (b"bXYZ", xyz_tag(BLUE)),
        (b"rTRC", curve),
    ];
    let shared_trc: [&[u8; 4]; 2] = [b"gTRC", b"bTRC"];
    let tag_count = tags.len() + shared_trc.len();

    let header_len = 128;
    let table_len = 4 + 12 * tag_count;
    let mut data = Vec::new();
    let mut table = Vec::with_capacity(table_len);
    table.extend_from_slice(&u32::try_from(tag_count).unwrap_or(0).to_be_bytes());
    let mut trc_entry = [0_u8; 8];
    for (sig, body) in &tags {
        let offset = header_len + table_len + data.len();
        let entry_off = u32::try_from(offset).unwrap_or(0).to_be_bytes();
        let entry_len = u32::try_from(body.len()).unwrap_or(0).to_be_bytes();
        table.extend_from_slice(*sig);
        table.extend_from_slice(&entry_off);
        table.extend_from_slice(&entry_len);
        if *sig == b"rTRC" {
            trc_entry[..4].copy_from_slice(&entry_off);
            trc_entry[4..].copy_from_slice(&entry_len);
        }
        data.extend_from_slice(body);
        while data.len() % 4 != 0 {
            data.push(0);
        }
    }
    for sig in shared_trc {
        table.extend_from_slice(sig);
        table.extend_from_slice(&trc_entry);
    }

    let total = header_len + table.len() + data.len();
    let mut header = Vec::with_capacity(header_len);
    header.extend_from_slice(&u32::try_from(total).unwrap_or(0).to_be_bytes()); // size
    header.extend_from_slice(&[0; 4]); // preferred CMM
    header.extend_from_slice(&[0x02, 0x10, 0, 0]); // version 2.1
    header.extend_from_slice(b"mntr"); // display class
    header.extend_from_slice(b"RGB "); // data colour space
    header.extend_from_slice(b"XYZ "); // connection space
    header.extend_from_slice(&[0x07, 0xEA, 0, 10, 0, 26, 0, 0, 0, 0, 0, 0]); // 2026-10-26 00:00:00
    header.extend_from_slice(b"acsp");
    header.extend_from_slice(&[0; 4]); // platform
    header.extend_from_slice(&[0; 4]); // flags
    header.extend_from_slice(&[0; 4]); // manufacturer
    header.extend_from_slice(&[0; 4]); // model
    header.extend_from_slice(&[0; 8]); // attributes
    header.extend_from_slice(&[0; 4]); // rendering intent: perceptual
    for c in D50 {
        header.extend_from_slice(&s15f16(c)); // illuminant
    }
    header.extend_from_slice(&[0; 4]); // creator
    header.resize(header_len, 0); // profile id + reserved

    let mut profile = header;
    profile.extend_from_slice(&table);
    profile.extend_from_slice(&data);
    profile
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be32(b: &[u8], at: usize) -> usize {
        u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as usize
    }

    #[test]
    fn the_header_declares_the_right_size_class_and_signature() {
        let p = srgb_profile();
        assert_eq!(be32(&p, 0), p.len());
        assert_eq!(&p[12..20], b"mntrRGB ");
        assert_eq!(&p[20..24], b"XYZ ");
        assert_eq!(&p[36..40], b"acsp");
    }

    #[test]
    fn every_tag_points_inside_the_profile_at_its_declared_type() {
        let p = srgb_profile();
        let count = be32(&p, 128);
        assert_eq!(count, 9);
        let mut sigs = Vec::new();
        for i in 0..count {
            let e = 132 + 12 * i;
            let (off, len) = (be32(&p, e + 4), be32(&p, e + 8));
            assert!(off % 4 == 0 && off + len <= p.len(), "tag {i}");
            sigs.push(String::from_utf8_lossy(&p[e..e + 4]).into_owned());
            let kind = &p[off..off + 4];
            assert!([b"desc", b"text", b"XYZ ", b"curv"].iter().any(|k| *k == kind), "tag {i} type {kind:?}");
        }
        for want in ["desc", "cprt", "wtpt", "rXYZ", "gXYZ", "bXYZ", "rTRC", "gTRC", "bTRC"] {
            assert!(sigs.iter().any(|s| s == want), "missing {want}");
        }
    }

    #[test]
    fn the_colorants_add_up_to_the_d50_white() {
        for c in 0..3 {
            let sum = RED[c] + GREEN[c] + BLUE[c];
            assert!((sum - D50[c]).abs() < 1e-3, "component {c}: {sum}");
        }
    }

    #[test]
    fn s15f16_encodes_one_and_a_half() {
        assert_eq!(s15f16(1.5), [0, 1, 0x80, 0]);
        assert_eq!(s15f16(-1.0), [0xFF, 0xFF, 0, 0]);
    }
}
