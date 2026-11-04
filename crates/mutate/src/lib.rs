//! Damaged copies of a valid file, for the tests of the parsers.
//!
//! Every parser that reads files made elsewhere gets, in its own tests,
//! thousands of copies of a valid sample with bytes flipped, overwritten
//! with boundary values, removed, repeated or cut off, and must answer each
//! with a value or an error, never a panic. The copies come from a fixed
//! seed, so a failure is the same on every run and on every computer; the
//! coverage-guided fuzzers in `fuzz/` look further on the same entry points.
//!
//! A dev-dependency only: nothing of it is in the programs.

/// `count` damaged copies of `sample`, the same ones for the same seed.
#[must_use]
pub fn copies(sample: &[u8], seed: u64, count: usize) -> Copies<'_> {
    Copies { sample, state: seed.max(1), left: count }
}

/// Iterator of [`copies`].
#[derive(Debug, Clone)]
pub struct Copies<'a> {
    sample: &'a [u8],
    state: u64,
    left: usize,
}

/// Values that sit on the boundaries of the integer types: where lengths,
/// counts and offsets overflow or become negative.
const BOUNDARIES: [u8; 8] = [0x00, 0x01, 0x7F, 0x80, 0xFE, 0xFF, 0x10, 0x40];

impl Copies<'_> {
    /// xorshift64*: fast and reproducible (not for anything secret).
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number in `0..n` (`n > 0`).
    fn below(&mut self, n: usize) -> usize {
        let n64 = u64::try_from(n).unwrap_or(u64::MAX);
        usize::try_from(self.next_u64() % n64).unwrap_or(0)
    }

    /// One change to `data`.
    fn damage(&mut self, data: &mut Vec<u8>) {
        if data.is_empty() {
            data.push(self.next_u64().to_le_bytes()[0]);
            return;
        }
        let at = self.below(data.len());
        match self.below(7) {
            0 => data[at] ^= 1 << self.below(8),
            1 => data[at] = BOUNDARIES[self.below(BOUNDARIES.len())],
            2 => {
                // A 32-bit boundary value, as a length or an offset would be.
                let value: u32 = [0, 1, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFF, 0xFFFF][self.below(6)];
                for (i, b) in value.to_le_bytes().into_iter().enumerate() {
                    if let Some(slot) = data.get_mut(at + i) {
                        *slot = b;
                    }
                }
            }
            3 => {
                let len = 1 + self.below(16.min(data.len() - at));
                data.drain(at..at + len);
            }
            4 => {
                let len = 1 + self.below(16.min(data.len() - at));
                let piece = data[at..at + len].to_vec();
                let to = self.below(data.len() + 1);
                data.splice(to..to, piece);
            }
            5 => data.truncate(at),
            _ => data[at] = self.next_u64().to_le_bytes()[0],
        }
    }
}

impl Iterator for Copies<'_> {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Vec<u8>> {
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        let mut data = self.sample.to_vec();
        for _ in 0..=self.below(4) {
            self.damage(&mut data);
        }
        Some(data)
    }
}

/// [`copies`] as text: damaged copies of a text sample, read back as UTF-8
/// where possible (the rest replaced), for parsers that take `&str`.
#[must_use]
pub fn text_copies(sample: &str, seed: u64, count: usize) -> Vec<String> {
    copies(sample.as_bytes(), seed, count).map(|b| String::from_utf8_lossy(&b).into_owned()).collect()
}

/// Checks that the fuzzers' starting file `fuzz/seeds/<target>/<name>` is
/// `sample`, a valid file the test made. With `AKTIS_WRITE_SEEDS` set it
/// writes the file instead (after a change of format).
///
/// # Panics
/// When the file differs from `sample` or cannot be read or written.
pub fn check_seed(target: &str, name: &str, sample: &[u8]) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds").join(target).join(name);
    if std::env::var_os("AKTIS_WRITE_SEEDS").is_some() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("seed folder");
        }
        std::fs::write(&path, sample).expect("seed written");
    }
    let seed = std::fs::read(&path).unwrap_or_default();
    assert!(
        seed == sample,
        "fuzz/seeds/{target}/{name} is not the current format: run the tests with AKTIS_WRITE_SEEDS=1"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copies_are_reproducible_and_differ_from_the_sample() {
        let sample = b"0123456789abcdefghij";
        let a: Vec<Vec<u8>> = copies(sample, 7, 200).collect();
        let b: Vec<Vec<u8>> = copies(sample, 7, 200).collect();
        assert_eq!(a, b);
        assert_eq!(a.len(), 200);
        assert!(a.iter().filter(|c| c.as_slice() != sample).count() > 150);
        assert!(a.iter().any(|c| c.len() < sample.len()) && a.iter().any(|c| c.len() > sample.len()));
        assert_ne!(a, copies(sample, 8, 200).collect::<Vec<_>>());
    }

    #[test]
    fn an_empty_sample_still_gives_copies() {
        assert_eq!(copies(b"", 1, 10).count(), 10);
        assert!(copies(b"", 1, 10).any(|c| !c.is_empty()));
        assert_eq!(text_copies("", 1, 5).len(), 5);
    }
}
