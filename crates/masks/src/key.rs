//! Stable keys for AI rasters.
//!
//! The key must be the same on every machine and every build, because it
//! names files in the raster cache and is stored in recipes. So it uses
//! FNV-1a 128 over an explicit byte encoding, never `std`'s hasher (whose
//! output may change between Rust versions).

use core_model::{AiTarget, ModelId, RasterKey};

/// Bumped when the encoding below changes, so old keys stop matching.
const ENCODING_VERSION: u8 = 1;

/// FNV-1a, 128-bit.
#[derive(Debug, Clone)]
struct Fnv128(u128);

impl Fnv128 {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

    fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u128::from(*b);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    /// A length-prefixed string, so ("ab", "c") and ("a", "bc") differ.
    fn str(&mut self, s: &str) {
        self.bytes(&(s.len() as u64).to_le_bytes());
        self.bytes(s.as_bytes());
    }

    fn f32(&mut self, v: f32) {
        // -0.0 and 0.0 are the same position.
        let v = if v == 0.0 { 0.0_f32 } else { v };
        self.bytes(&v.to_bits().to_le_bytes());
    }

    fn rect(&mut self, r: core_model::NormRect) {
        for v in [r.min().x(), r.min().y(), r.max().x(), r.max().y()] {
            self.f32(v);
        }
    }

    fn finish(&self) -> [u8; 16] {
        self.0.to_be_bytes()
    }
}

/// The key of the raster `model` produces for `target` on the image
/// identified by `image_fingerprint` (for example a hash of the RAW file and
/// of the development settings the model saw).
#[must_use]
pub fn raster_key(image_fingerprint: &str, model: &ModelId, target: &AiTarget) -> RasterKey {
    let mut h = Fnv128::new();
    h.bytes(&[ENCODING_VERSION]);
    h.str(image_fingerprint);
    h.str(model.as_str());
    match target {
        AiTarget::Subject => h.bytes(&[0]),
        AiTarget::Object { points, rect } => {
            h.bytes(&[1]);
            h.bytes(&(points.len() as u64).to_le_bytes());
            for p in points {
                h.f32(p.at.x());
                h.f32(p.at.y());
                h.bytes(&[u8::from(p.include)]);
            }
            match rect {
                None => h.bytes(&[0]),
                Some(r) => {
                    h.bytes(&[1]);
                    h.rect(*r);
                }
            }
        }
        AiTarget::PersonPart { person, part } => {
            h.bytes(&[2]);
            h.rect(*person);
            h.str(part.id());
        }
    }
    RasterKey::from_bytes(h.finish())
}

/// A fingerprint of arbitrary bytes (a file's contents, settings), as 32 hex
/// digits, for use as the `image_fingerprint` of [`raster_key`].
#[must_use]
pub fn fingerprint(parts: &[&[u8]]) -> String {
    let mut h = Fnv128::new();
    for p in parts {
        h.bytes(&(p.len() as u64).to_le_bytes());
        h.bytes(p);
    }
    RasterKey::from_bytes(h.finish()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{NormPoint, NormRect, PromptPoint};

    fn model() -> ModelId {
        ModelId::new("sam2.1_small").expect("valid")
    }

    fn click(x: f32, y: f32, include: bool) -> PromptPoint {
        PromptPoint { at: NormPoint::new(x, y).expect("finite"), include }
    }

    #[test]
    fn fnv1a_128_matches_the_reference_vectors() {
        let mut h = Fnv128::new();
        assert_eq!(h.0, Fnv128::OFFSET);
        h.bytes(b"a");
        assert_eq!(h.0, 0xd228_cb69_6f1a_8caf_7891_2b70_4e4a_8964, "FNV-1a 128 of \"a\"");
    }

    #[test]
    fn keys_are_stable_and_depend_on_every_input() {
        let t = AiTarget::Object { points: vec![click(0.5, 0.25, true)], rect: None };
        let k = raster_key("img", &model(), &t);
        assert_eq!(k, raster_key("img", &model(), &t));
        assert_ne!(k, raster_key("img2", &model(), &t));
        assert_ne!(k, raster_key("img", &ModelId::new("sam2.1_large").expect("valid"), &t));
        assert_ne!(k, raster_key("img", &model(), &AiTarget::Subject));
        let excluded = AiTarget::Object { points: vec![click(0.5, 0.25, false)], rect: None };
        assert_ne!(k, raster_key("img", &model(), &excluded));
        let rect = NormRect::new(NormPoint::new(0.1, 0.1).expect("p"), NormPoint::new(0.2, 0.3).expect("p"));
        let boxed = AiTarget::Object { points: vec![click(0.5, 0.25, true)], rect: Some(rect) };
        assert_ne!(k, raster_key("img", &model(), &boxed));
        let person = NormRect::new(NormPoint::new(0.1, 0.1).expect("p"), NormPoint::new(0.4, 0.9).expect("p"));
        let hair = AiTarget::PersonPart { person, part: core_model::BodyPart::Hair };
        let lips = AiTarget::PersonPart { person, part: core_model::BodyPart::Lips };
        assert_ne!(raster_key("img", &model(), &hair), raster_key("img", &model(), &lips));
    }

    #[test]
    fn string_boundaries_matter() {
        assert_ne!(fingerprint(&[b"ab", b"c"]), fingerprint(&[b"a", b"bc"]));
        assert_eq!(fingerprint(&[b"x"]).len(), 32);
    }
}
