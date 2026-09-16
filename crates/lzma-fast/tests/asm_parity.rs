//! The assembly decode loop against the portable one.
//!
//! `Asm/arm64/LzmaDecOpt.S` and `Asm/x86/LzmaDecOpt.asm` are drop-in
//! replacements for `LzmaDec_DecodeReal_3`, so the two loops must agree on
//! every input, byte for byte and error for error — on well-formed streams,
//! on truncated ones, and on corrupt ones. That is what this file checks. On
//! a build with no assembly loop the two sides are the same code and the
//! tests still pass, which is the point: the file never needs a `cfg`.

mod common;

use common::*;

/// Chunk pairs chosen to land the decoder in both loops' entry conditions:
/// the one-byte lanes force the `tempBuf` single-symbol path, where the
/// assembly is entered with `bufLimit == buf`, and the large ones let it run
/// its own limit checks for millions of symbols.
const CHUNKS: &[(usize, usize)] = &[(1, 1), (7, 13), (4096, 4096), (usize::MAX, 1 << 20)];

#[test]
fn asm_and_portable_agree_on_lzma1_vectors() {
    for stem in SOURCES {
        for variant in LZMA_VARIANTS {
            let name = format!("{stem}.{variant}.lzma");
            let data = read(&name);
            for &(in_chunk, out_chunk) in CHUNKS {
                let asm = decode_lzma_with(&data, in_chunk, out_chunk, false);
                let portable = decode_lzma_with(&data, in_chunk, out_chunk, true);
                match (asm, portable) {
                    (Ok(a), Ok(b)) => {
                        assert_eq!(a.bytes, b.bytes, "{name} ({in_chunk},{out_chunk}) bytes");
                        assert_eq!(a.status, b.status, "{name} ({in_chunk},{out_chunk}) status");
                    }
                    (a, b) => panic!("{name} ({in_chunk},{out_chunk}): {a:?} vs {b:?}"),
                }
            }
        }
    }
}

#[test]
fn asm_and_portable_agree_on_lzma2_vectors() {
    for stem in SOURCES {
        for variant in XZ_VARIANTS {
            let name = format!("{stem}.{variant}.xz");
            let data = read(&name);
            let Some((dict_prop, payload)) = xz_lzma2_block(&data) else {
                continue;
            };
            for &(in_chunk, out_chunk) in CHUNKS {
                let asm = decode_lzma2_with(dict_prop, payload, in_chunk, out_chunk, false);
                let portable = decode_lzma2_with(dict_prop, payload, in_chunk, out_chunk, true);
                match (asm, portable) {
                    (Ok(a), Ok(b)) => {
                        assert_eq!(a.bytes, b.bytes, "{name} ({in_chunk},{out_chunk}) bytes");
                        assert_eq!(a.status, b.status, "{name} ({in_chunk},{out_chunk}) status");
                    }
                    (a, b) => panic!("{name} ({in_chunk},{out_chunk}): {a:?} vs {b:?}"),
                }
            }
        }
    }
}

#[test]
fn asm_and_portable_agree_on_every_truncation() {
    for stem in SOURCES {
        for variant in LZMA_VARIANTS {
            let name = format!("{stem}.{variant}.lzma");
            let data = read(&name);
            for cut in truncation_points(data.len()) {
                let prefix = &data[..cut];
                let asm = decode_lzma_with(prefix, usize::MAX, 1 << 16, false);
                let portable = decode_lzma_with(prefix, usize::MAX, 1 << 16, true);
                match (asm, portable) {
                    (Ok(a), Ok(b)) => {
                        assert_eq!(a.bytes, b.bytes, "{name} cut at {cut}: bytes");
                        assert_eq!(a.status, b.status, "{name} cut at {cut}: status");
                    }
                    (Err(a), Err(b)) => assert_eq!(a, b, "{name} cut at {cut}: error"),
                    (a, b) => panic!("{name} cut at {cut}: {a:?} vs {b:?}"),
                }
            }
        }
    }
}

/// Where to cut a stream of `len` bytes.
///
/// Every prefix of the first 512 bytes, because that is where the header, the
/// range coder's first five bytes and the first symbols are and where an
/// off-by-one in the input margin shows; then a stride over the rest, bounded
/// so that a vector costs a few hundred decodes rather than one per byte. A
/// decoder allocates its whole dictionary, and `p9e` asks for 64 MiB of it,
/// so "every cut of every vector" is hours of `mmap` on Linux and proves
/// nothing the stride does not.
fn truncation_points(len: usize) -> impl Iterator<Item = usize> {
    let dense = len.min(512);
    let stride = (len.saturating_sub(dense) / 256).max(1);
    (0..dense).chain((dense..len).step_by(stride))
}

#[test]
fn asm_and_portable_agree_on_corrupt_streams() {
    // Same generator as `robustness.rs`, so the two files flip the same bytes.
    let mut rng = Rng::new(0x5EED_1234_ABCD_9876);
    for stem in SOURCES {
        let name = format!("{stem}.p1.lzma");
        let data = read(&name);
        if data.len() <= 13 {
            continue;
        }
        for _ in 0..400 {
            let mut bad = data.clone();
            let at = 13 + (rng.next() as usize) % (bad.len() - 13);
            bad[at] ^= 1 << (rng.next() % 8);
            let asm = decode_lzma_with(&bad, usize::MAX, 1 << 16, false);
            let portable = decode_lzma_with(&bad, usize::MAX, 1 << 16, true);
            match (asm, portable) {
                (Ok(a), Ok(b)) => {
                    assert_eq!(a.bytes, b.bytes, "{name} flip at {at}: bytes");
                    assert_eq!(a.status, b.status, "{name} flip at {at}: status");
                }
                (Err(a), Err(b)) => assert_eq!(a, b, "{name} flip at {at}: error"),
                (a, b) => panic!("{name} flip at {at}: {a:?} vs {b:?}"),
            }
        }
    }
}

/// splitmix64, so the corruption cases are reproducible without a dev
/// dependency.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }

    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 32) as u32
    }
}
