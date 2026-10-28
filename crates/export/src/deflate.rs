//! A small zlib compressor (RFC 1950/1951) for PNG files and their ICC
//! profile: LZ77 matching on hash chains, then dynamic Huffman blocks (or
//! stored blocks when the data does not compress).
//!
//! Written here so the export needs no compression crate; the tests carry a
//! small decompressor that checks every stream round-trips.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Positions of the sliding window (the largest distance deflate allows).
const WINDOW: usize = 1 << 15;
const WINDOW_MASK: usize = WINDOW - 1;
const HASH_BITS: u32 = 15;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
/// Earlier positions tried per match search: speed against compression.
const MAX_CHAIN: usize = 32;
/// A match this long is taken without looking for a longer one.
const NICE_MATCH: usize = 96;
/// Symbols per block: each block gets Huffman codes fitted to its own data.
const BLOCK_SYMBOLS: usize = 1 << 16;
const NONE: usize = usize::MAX;

const LEN_BASE: [u16; 29] =
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
/// Order in which the code-length code lengths are sent.
const CL_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// A literal byte, or a match of `len` bytes `dist` bytes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Symbol {
    Literal(u8),
    Match { len: u16, dist: u16 },
}

/// Bits written least significant first, as deflate wants.
struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    bits: u32,
}

impl BitWriter {
    fn put(&mut self, value: u32, count: u32) {
        debug_assert!(count <= 32);
        self.acc |= u64::from(value) << self.bits;
        self.bits += count;
        while self.bits >= 8 {
            self.out.push(self.acc.to_le_bytes()[0]);
            self.acc >>= 8;
            self.bits -= 8;
        }
    }

    fn align(&mut self) {
        if self.bits > 0 {
            self.put(0, 8 - self.bits);
        }
    }
}

/// `data` compressed as a zlib stream (header, deflate blocks, Adler-32).
#[must_use]
pub fn zlib_compress(data: &[u8]) -> Vec<u8> {
    let mut w = BitWriter { out: Vec::with_capacity(data.len() / 2 + 64), acc: 0, bits: 0 };
    // CMF 0x78: deflate, 32 KB window; FLG 0x9C: default level, check bits.
    w.out.extend_from_slice(&[0x78, 0x9C]);
    deflate(data, &mut w);
    w.align();
    w.out.extend_from_slice(&adler32(data).to_be_bytes());
    w.out
}

/// The Adler-32 checksum of zlib.
#[must_use]
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    // 5552 bytes is the most that can be summed before the sums can overflow.
    let (mut a, mut b) = (1_u32, 0_u32);
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

fn hash(data: &[u8], i: usize) -> usize {
    let v = u32::from(data[i]) | u32::from(data[i + 1]) << 8 | u32::from(data[i + 2]) << 16;
    (v.wrapping_mul(2_654_435_761) >> (32 - HASH_BITS)) as usize
}

fn deflate(data: &[u8], w: &mut BitWriter) {
    let mut head = vec![NONE; 1 << HASH_BITS];
    let mut prev = vec![NONE; WINDOW];
    let mut symbols: Vec<Symbol> = Vec::with_capacity(BLOCK_SYMBOLS);
    let mut block_start = 0;
    let mut i = 0;
    let n = data.len();
    let insert = |head: &mut [usize], prev: &mut [usize], pos: usize| {
        if pos + MIN_MATCH <= n {
            let h = hash(data, pos);
            prev[pos & WINDOW_MASK] = head[h];
            head[h] = pos;
        }
    };
    while i < n {
        let (mut best_len, mut best_dist) = (0, 0);
        if i + MIN_MATCH <= n {
            let max_len = MAX_MATCH.min(n - i);
            let mut candidate = head[hash(data, i)];
            let mut chain = 0;
            while candidate != NONE && i - candidate <= WINDOW && chain < MAX_CHAIN {
                if data[candidate + best_len.min(max_len - 1)] == data[i + best_len.min(max_len - 1)] {
                    let len = data[candidate..candidate + max_len]
                        .iter()
                        .zip(&data[i..i + max_len])
                        .take_while(|(a, b)| a == b)
                        .count();
                    if len > best_len {
                        best_len = len;
                        best_dist = i - candidate;
                        if len >= NICE_MATCH.min(max_len) {
                            break;
                        }
                    }
                }
                let next = prev[candidate & WINDOW_MASK];
                // A slot reused by a newer position ends the chain.
                if next == NONE || next >= candidate {
                    break;
                }
                candidate = next;
                chain += 1;
            }
        }
        if best_len >= MIN_MATCH {
            #[allow(clippy::cast_possible_truncation)] // len ≤ 258, dist ≤ 32768
            symbols.push(Symbol::Match { len: best_len as u16, dist: best_dist as u16 });
            for pos in i..i + best_len {
                insert(&mut head, &mut prev, pos);
            }
            i += best_len;
        } else {
            symbols.push(Symbol::Literal(data[i]));
            insert(&mut head, &mut prev, i);
            i += 1;
        }
        if symbols.len() >= BLOCK_SYMBOLS {
            write_block(&symbols, &data[block_start..i], i == n, w);
            symbols.clear();
            block_start = i;
        }
    }
    if !symbols.is_empty() || block_start == 0 {
        write_block(&symbols, &data[block_start..], true, w);
    }
}

fn len_code(len: u16) -> usize {
    LEN_BASE.partition_point(|&b| b <= len) - 1
}

fn dist_code(dist: u16) -> usize {
    DIST_BASE.partition_point(|&b| b <= dist) - 1
}

/// Writes `symbols` (which encode `raw`) as one dynamic Huffman block, or
/// as stored blocks when that is smaller.
fn write_block(symbols: &[Symbol], raw: &[u8], last: bool, w: &mut BitWriter) {
    let mut lit_freq = [0_u32; 286];
    let mut dist_freq = [0_u32; 30];
    for s in symbols {
        match *s {
            Symbol::Literal(b) => lit_freq[usize::from(b)] += 1,
            Symbol::Match { len, dist } => {
                lit_freq[257 + len_code(len)] += 1;
                dist_freq[dist_code(dist)] += 1;
            }
        }
    }
    lit_freq[256] = 1;
    // Two codes at least in each tree, so every tree is complete.
    for freq in [&mut lit_freq[..], &mut dist_freq[..]] {
        let mut used = freq.iter().filter(|&&f| f > 0).count();
        for f in freq.iter_mut() {
            if used >= 2 {
                break;
            }
            if *f == 0 {
                *f = 1;
                used += 1;
            }
        }
    }
    let lit_len = huffman_lengths(&lit_freq, 15);
    let dist_len = huffman_lengths(&dist_freq, 15);
    let hlit = 257.max(lit_len.iter().rposition(|&l| l > 0).map_or(0, |p| p + 1));
    let hdist = 1.max(dist_len.iter().rposition(|&l| l > 0).map_or(0, |p| p + 1));
    let mut lengths = lit_len[..hlit].to_vec();
    lengths.extend_from_slice(&dist_len[..hdist]);
    let runs = run_length(&lengths);
    let mut cl_freq = [0_u32; 19];
    for &(sym, _) in &runs {
        cl_freq[usize::from(sym)] += 1;
    }
    let cl_len = huffman_lengths(&cl_freq, 7);
    let hclen = 4.max(CL_ORDER.iter().rposition(|&s| cl_len[s] > 0).map_or(0, |p| p + 1));

    // Size of both choices, in bits.
    let extra_bits = |sym: u8| match sym {
        16 => 2,
        17 => 3,
        18 => 7,
        _ => 0,
    };
    let mut dynamic_bits: u64 = 3 + 14 + 3 * hclen as u64;
    for &(sym, _) in &runs {
        dynamic_bits += u64::from(cl_len[usize::from(sym)]) + extra_bits(sym);
    }
    for (code, f) in lit_freq.iter().enumerate() {
        let extra = if code > 256 { u64::from(LEN_EXTRA[code - 257]) } else { 0 };
        dynamic_bits += u64::from(*f) * (u64::from(lit_len[code]) + extra);
    }
    for (code, f) in dist_freq.iter().enumerate() {
        dynamic_bits += u64::from(*f) * (u64::from(dist_len[code]) + u64::from(DIST_EXTRA[code]));
    }
    let stored_blocks = raw.len().div_ceil(0xFFFF).max(1) as u64;
    let stored_bits = raw.len() as u64 * 8 + stored_blocks * (3 + 7 + 32);
    if stored_bits < dynamic_bits {
        write_stored(raw, last, w);
        return;
    }

    w.put(u32::from(last), 1);
    w.put(2, 2);
    #[allow(clippy::cast_possible_truncation)] // hlit ≤ 286, hdist ≤ 30, hclen ≤ 19
    {
        w.put((hlit - 257) as u32, 5);
        w.put((hdist - 1) as u32, 5);
        w.put((hclen - 4) as u32, 4);
    }
    for &s in &CL_ORDER[..hclen] {
        w.put(u32::from(cl_len[s]), 3);
    }
    let cl_codes = canonical_codes(&cl_len);
    for &(sym, extra) in &runs {
        let s = usize::from(sym);
        w.put(cl_codes[s], u32::from(cl_len[s]));
        let bits = extra_bits(sym);
        if bits > 0 {
            #[allow(clippy::cast_possible_truncation)] // 2, 3 or 7
            w.put(u32::from(extra), bits as u32);
        }
    }
    let lit_codes = canonical_codes(&lit_len);
    let dist_codes = canonical_codes(&dist_len);
    for s in symbols {
        match *s {
            Symbol::Literal(b) => {
                let c = usize::from(b);
                w.put(lit_codes[c], u32::from(lit_len[c]));
            }
            Symbol::Match { len, dist } => {
                let lc = len_code(len);
                w.put(lit_codes[257 + lc], u32::from(lit_len[257 + lc]));
                w.put(u32::from(len - LEN_BASE[lc]), u32::from(LEN_EXTRA[lc]));
                let dc = dist_code(dist);
                w.put(dist_codes[dc], u32::from(dist_len[dc]));
                w.put(u32::from(dist - DIST_BASE[dc]), u32::from(DIST_EXTRA[dc]));
            }
        }
    }
    w.put(lit_codes[256], u32::from(lit_len[256]));
}

fn write_stored(raw: &[u8], last: bool, w: &mut BitWriter) {
    let mut chunks = raw.chunks(0xFFFF).peekable();
    if chunks.peek().is_none() {
        w.put(u32::from(last), 1);
        w.put(0, 2);
        w.align();
        w.out.extend_from_slice(&[0, 0, 0xFF, 0xFF]);
        return;
    }
    while let Some(chunk) = chunks.next() {
        let final_chunk = last && chunks.peek().is_none();
        w.put(u32::from(final_chunk), 1);
        w.put(0, 2);
        w.align();
        #[allow(clippy::cast_possible_truncation)] // chunks of at most 65 535 bytes
        let len = chunk.len() as u16;
        w.out.extend_from_slice(&len.to_le_bytes());
        w.out.extend_from_slice(&(!len).to_le_bytes());
        w.out.extend_from_slice(chunk);
    }
}

/// The code-length sequence with the repeat codes 16, 17 and 18: (symbol,
/// extra bits value).
fn run_length(lengths: &[u8]) -> Vec<(u8, u8)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lengths.len() {
        let v = lengths[i];
        let run = lengths[i..].iter().take_while(|&&l| l == v).count();
        let mut left = run;
        if v == 0 {
            while left >= 11 {
                let r = left.min(138);
                #[allow(clippy::cast_possible_truncation)] // r - 11 ≤ 127
                out.push((18, (r - 11) as u8));
                left -= r;
            }
            if left >= 3 {
                #[allow(clippy::cast_possible_truncation)] // left - 3 ≤ 7
                out.push((17, (left - 3) as u8));
                left = 0;
            }
        } else {
            out.push((v, 0));
            left -= 1;
            while left >= 3 {
                let r = left.min(6);
                #[allow(clippy::cast_possible_truncation)] // r - 3 ≤ 3
                out.push((16, (r - 3) as u8));
                left -= r;
            }
        }
        out.extend(std::iter::repeat_n((v, 0), left));
        i += run;
    }
    out
}

/// Huffman code lengths for `freq`, none longer than `limit`. Frequencies
/// are halved until the tree is shallow enough (they converge on a
/// balanced tree, which always fits).
fn huffman_lengths(freq: &[u32], limit: u8) -> Vec<u8> {
    let mut f = freq.to_vec();
    loop {
        let lengths = tree_depths(&f);
        if lengths.iter().all(|&l| l <= limit) {
            return lengths;
        }
        for x in &mut f {
            if *x > 0 {
                *x = (*x >> 1).max(1);
            }
        }
    }
}

/// Depth of each used symbol in a Huffman tree of `freq`.
fn tree_depths(freq: &[u32]) -> Vec<u8> {
    let mut lengths = vec![0_u8; freq.len()];
    // Nodes: leaves first (their symbol), then inner nodes (two children).
    let mut children: Vec<Option<(usize, usize)>> = Vec::new();
    let mut symbol_of: Vec<usize> = Vec::new();
    let mut heap = BinaryHeap::new();
    for (s, &f) in freq.iter().enumerate() {
        if f > 0 {
            heap.push(Reverse((u64::from(f), children.len())));
            children.push(None);
            symbol_of.push(s);
        }
    }
    match heap.len() {
        0 => return lengths,
        1 => {
            lengths[symbol_of[0]] = 1;
            return lengths;
        }
        _ => {}
    }
    while heap.len() > 1 {
        let (Some(Reverse((wa, a))), Some(Reverse((wb, b)))) = (heap.pop(), heap.pop()) else { break };
        heap.push(Reverse((wa + wb, children.len())));
        children.push(Some((a, b)));
    }
    let root = children.len() - 1;
    let mut stack = vec![(root, 0_u8)];
    while let Some((node, depth)) = stack.pop() {
        match children[node] {
            Some((a, b)) => {
                stack.push((a, depth.saturating_add(1)));
                stack.push((b, depth.saturating_add(1)));
            }
            None => lengths[symbol_of[node]] = depth,
        }
    }
    lengths
}

/// Canonical Huffman codes for `lengths`, bit-reversed for writing least
/// significant bit first.
fn canonical_codes(lengths: &[u8]) -> Vec<u32> {
    let mut count = [0_u32; 16];
    for &l in lengths {
        count[usize::from(l)] += 1;
    }
    count[0] = 0;
    let mut next = [0_u32; 16];
    let mut code = 0;
    for bits in 1..16 {
        code = (code + count[bits - 1]) << 1;
        next[bits] = code;
    }
    lengths
        .iter()
        .map(|&l| {
            if l == 0 {
                return 0;
            }
            let c = next[usize::from(l)];
            next[usize::from(l)] += 1;
            c.reverse_bits() >> (32 - u32::from(l))
        })
        .collect()
}

/// A small inflater for the tests (stored, fixed and dynamic blocks).
#[cfg(test)]
pub(crate) mod inflate {
    struct Bits<'a> {
        data: &'a [u8],
        pos: usize,
        bit: u32,
    }

    impl Bits<'_> {
        fn get(&mut self, n: u32) -> Result<u32, String> {
            let mut v = 0;
            for k in 0..n {
                let byte = *self.data.get(self.pos).ok_or("out of data")?;
                v |= u32::from((byte >> self.bit) & 1) << k;
                self.bit += 1;
                if self.bit == 8 {
                    self.bit = 0;
                    self.pos += 1;
                }
            }
            Ok(v)
        }
    }

    /// (length, code) → symbol, decoded bit by bit.
    struct Table {
        by_len: Vec<Vec<(u32, usize)>>,
    }

    impl Table {
        fn new(lengths: &[u8]) -> Self {
            let mut by_len = vec![Vec::new(); 16];
            let mut code = 0_u32;
            for len in 1..16_u8 {
                for (sym, &l) in lengths.iter().enumerate() {
                    if l == len {
                        by_len[usize::from(len)].push((code, sym));
                        code += 1;
                    }
                }
                code <<= 1;
            }
            Self { by_len }
        }

        fn decode(&self, b: &mut Bits<'_>) -> Result<usize, String> {
            let mut code = 0;
            for len in 1..16 {
                code = (code << 1) | b.get(1)?;
                if let Some(&(_, sym)) = self.by_len[len].iter().find(|(c, _)| *c == code) {
                    return Ok(sym);
                }
            }
            Err("bad code".into())
        }
    }

    /// Decompresses a zlib stream and checks its Adler-32.
    pub(crate) fn zlib_decompress(data: &[u8]) -> Result<Vec<u8>, String> {
        if data.len() < 6 || (u16::from(data[0]) << 8 | u16::from(data[1])) % 31 != 0 || data[0] & 0x0F != 8 {
            return Err("bad zlib header".into());
        }
        let mut b = Bits { data: &data[2..], pos: 0, bit: 0 };
        let mut out: Vec<u8> = Vec::new();
        loop {
            let last = b.get(1)?;
            match b.get(2)? {
                0 => {
                    if b.bit > 0 {
                        b.bit = 0;
                        b.pos += 1;
                    }
                    let d = &b.data[b.pos..];
                    let len = usize::from(u16::from_le_bytes([d[0], d[1]]));
                    let nlen = u16::from_le_bytes([d[2], d[3]]);
                    if nlen != !u16::try_from(len).map_err(|e| e.to_string())? {
                        return Err("bad stored length".into());
                    }
                    out.extend_from_slice(&d[4..4 + len]);
                    b.pos += 4 + len;
                }
                kind @ (1 | 2) => {
                    let (lit, dist) = if kind == 1 {
                        let mut l = vec![8_u8; 288];
                        l[144..256].fill(9);
                        l[256..280].fill(7);
                        (Table::new(&l), Table::new(&[5; 30]))
                    } else {
                        let hlit = b.get(5)? as usize + 257;
                        let hdist = b.get(5)? as usize + 1;
                        let hclen = b.get(4)? as usize + 4;
                        let mut cl = [0_u8; 19];
                        for &s in &super::CL_ORDER[..hclen] {
                            cl[s] = u8::try_from(b.get(3)?).map_err(|e| e.to_string())?;
                        }
                        let cl_table = Table::new(&cl);
                        let mut lengths: Vec<u8> = Vec::new();
                        while lengths.len() < hlit + hdist {
                            let sym = cl_table.decode(&mut b)?;
                            match sym {
                                0..=15 => lengths.push(u8::try_from(sym).map_err(|e| e.to_string())?),
                                16 => {
                                    let p = *lengths.last().ok_or("repeat without a length")?;
                                    let r = 3 + b.get(2)? as usize;
                                    lengths.extend(std::iter::repeat_n(p, r));
                                }
                                17 => {
                                    let r = 3 + b.get(3)? as usize;
                                    lengths.extend(std::iter::repeat_n(0, r));
                                }
                                _ => {
                                    let r = 11 + b.get(7)? as usize;
                                    lengths.extend(std::iter::repeat_n(0, r));
                                }
                            }
                        }
                        (Table::new(&lengths[..hlit]), Table::new(&lengths[hlit..]))
                    };
                    loop {
                        let sym = lit.decode(&mut b)?;
                        match sym {
                            0..=255 => out.push(u8::try_from(sym).map_err(|e| e.to_string())?),
                            256 => break,
                            _ => {
                                let c = sym - 257;
                                let len =
                                    usize::from(super::LEN_BASE[c]) + b.get(u32::from(super::LEN_EXTRA[c]))? as usize;
                                let dc = dist.decode(&mut b)?;
                                let d = usize::from(super::DIST_BASE[dc])
                                    + b.get(u32::from(super::DIST_EXTRA[dc]))? as usize;
                                if d > out.len() {
                                    return Err("distance too far".into());
                                }
                                for _ in 0..len {
                                    out.push(out[out.len() - d]);
                                }
                            }
                        }
                    }
                }
                _ => return Err("bad block type".into()),
            }
            if last == 1 {
                break;
            }
        }
        if b.bit > 0 {
            b.pos += 1;
        }
        let tail = b.data.get(b.pos..b.pos + 4).ok_or("no checksum")?;
        if u32::from_be_bytes([tail[0], tail[1], tail[2], tail[3]]) != super::adler32(&out) {
            return Err("checksum mismatch".into());
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::inflate::zlib_decompress;
    use super::*;

    fn noise(n: usize, seed: u32) -> Vec<u8> {
        let mut x = seed.max(1);
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x.to_le_bytes()[0]
            })
            .collect()
    }

    fn round_trip(data: &[u8]) -> usize {
        let z = zlib_compress(data);
        assert_eq!(zlib_decompress(&z).expect("decompresses"), data);
        z.len()
    }

    #[test]
    fn empty_and_tiny_inputs_round_trip() {
        round_trip(&[]);
        round_trip(&[7]);
        round_trip(b"ab");
        round_trip(b"abc");
    }

    #[test]
    fn repetitive_data_compresses_well() {
        let text = b"the quick brown fox jumps over the lazy dog. ".repeat(500);
        let size = round_trip(&text);
        assert!(size < text.len() / 20, "{size} of {}", text.len());
        let zeros = vec![0_u8; 300_000];
        assert!(round_trip(&zeros) < 2_000);
    }

    #[test]
    fn noise_is_stored_without_growing_much() {
        let data = noise(200_000, 42);
        let size = round_trip(&data);
        assert!(size < data.len() + data.len() / 100, "{size}");
    }

    #[test]
    fn skewed_data_uses_dynamic_codes() {
        // Small values, as filtered photo rows give: entropy coding pays off.
        let data: Vec<u8> = noise(150_000, 9).iter().map(|b| b % 6).collect();
        let size = round_trip(&data);
        assert!(size < data.len() / 2, "{size}");
    }

    #[test]
    fn mixed_blocks_round_trip() {
        let mut data = noise(70_000, 3);
        data.extend(b"repeat me ".repeat(10_000));
        data.extend(noise(1000, 5));
        data.extend(std::iter::repeat_n(200_u8, 70_000));
        round_trip(&data);
    }

    #[test]
    fn code_lengths_respect_their_limit() {
        // Fibonacci frequencies make the deepest possible tree.
        let mut freq = vec![1_u32, 1];
        while freq.len() < 30 {
            let n = freq.len();
            freq.push(freq[n - 1] + freq[n - 2]);
        }
        let lengths = huffman_lengths(&freq, 15);
        assert!(lengths.iter().all(|&l| (1..=15).contains(&l)));
        // Kraft: a complete prefix code.
        let kraft: f64 = lengths.iter().map(|&l| 0.5_f64.powi(i32::from(l))).sum();
        assert!((kraft - 1.0).abs() < 1e-12, "{kraft}");
    }

    #[test]
    fn adler32_matches_the_known_value() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(&[]), 1);
    }
}
