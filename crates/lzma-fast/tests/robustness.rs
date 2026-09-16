//! Malformed input must return an error or ask for more input. It must never
//! panic and never read out of bounds.
//!
//! Run these under `cargo +nightly miri` or an ASan build to get the
//! out-of-bounds half; under a normal build they cover the panic half and the
//! "every `SZ_ERROR_DATA` path is reachable as an `Err`" half.

mod common;

use common::*;
use lzma_fast::Status;

/// A cheap deterministic PRNG so corruption cases are reproducible without a
/// dev-dependency. C: none; this is test scaffolding.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[test]
fn every_prefix_of_a_stream_is_safe() {
    for name in ["tiny.p9e.lzma", "mixed.p1.lzma", "zeros.lc0lp2pb0.lzma"] {
        let data = read(name);
        // Every prefix for the short vector, a stride for the longer ones.
        let stride = if data.len() < 512 {
            1
        } else {
            data.len() / 128 + 1
        };
        // One byte at a time is the slowest path there is; reserve it for the
        // vector small enough that every prefix of it is still cheap.
        let chunks: &[(usize, usize)] = if data.len() < 512 {
            &[(1, 1), (7, 4096), (usize::MAX, 4096)]
        } else {
            &[(7, 4096), (usize::MAX, 4096)]
        };
        let mut n = 0;
        while n <= data.len() {
            for &(in_chunk, out_chunk) in chunks {
                if let Ok(d) = decode_lzma(&data[..n], in_chunk, out_chunk) {
                    assert_ne!(
                        (d.status, n == data.len()),
                        (Status::FinishedWithMark, false),
                        "{name} prefix {n} claimed a clean end"
                    );
                }
            }
            n += stride;
        }
    }
}

#[test]
fn every_prefix_of_an_lzma2_stream_is_safe() {
    let data = read("mixed.p1.xz");
    let (dict_prop, payload) = xz_lzma2_block(&data).unwrap();
    let stride = payload.len() / 128 + 1;
    let mut n = 0;
    while n <= payload.len() {
        let _ = decode_lzma2(dict_prop, &payload[..n], usize::MAX, 4096);
        n += stride;
    }
}

#[test]
fn random_byte_flips_are_safe() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF0);
    for name in ["tiny.p9e.lzma", "mixed.p1.lzma", "text.lc4pb1.lzma"] {
        let original = read(name);
        for _ in 0..400 {
            let mut data = original.clone();
            let flips = 1 + (rng.next() % 3) as usize;
            for _ in 0..flips {
                let at = (rng.next() as usize) % data.len();
                data[at] ^= 1u8 << (rng.next() % 8);
            }
            // Any outcome is acceptable except a panic or a read out of bounds.
            let _ = decode_lzma(&data, usize::MAX, 4096);
            let _ = decode_lzma(&data, 3, 17);
        }
    }
}

#[test]
fn random_byte_flips_in_lzma2_are_safe() {
    let mut rng = Rng(0xFEED_FACE_CAFE_BEEF);
    let container = read("mixed.p1.xz");
    let (dict_prop, payload) = xz_lzma2_block(&container).unwrap();
    let payload = payload.to_vec();
    for _ in 0..400 {
        let mut data = payload.clone();
        let flips = 1 + (rng.next() % 3) as usize;
        for _ in 0..flips {
            let at = (rng.next() as usize) % data.len();
            data[at] ^= 1u8 << (rng.next() % 8);
        }
        let _ = decode_lzma2(dict_prop, &data, usize::MAX, 4096);
        let _ = decode_lzma2(dict_prop, &data, 5, 11);
    }
}

#[test]
fn arbitrary_bytes_never_panic() {
    let mut rng = Rng(0x0BAD_C0DE_0BAD_C0DE);
    for _ in 0..2000 {
        let len = (rng.next() % 256) as usize;
        let mut data = vec![0u8; len];
        for b in &mut data {
            *b = rng.next() as u8;
        }
        // Keep the dictionary small. A random header asks for a random
        // `dicSize` up to 4 GiB, and a decoder allocates it before it can
        // reject anything; 2000 of those is an out-of-memory test, not a
        // robustness one. The same clamp is in the fuzz targets.
        if data.len() >= 13 {
            data[3] = 0;
            data[4] = 0;
        }
        let _ = decode_lzma(&data, usize::MAX, 4096);
        let _ = decode_lzma2((rng.next() % 21) as u8, &data, usize::MAX, 4096);
    }
}

/// C: the `kBadRepCode` check in `LzmaDec_DecodeToDic`. A stream whose first
/// range-coder word is at or above `kBadRepCode` could only start with a rep
/// match, which is illegal, and must be rejected before the fast loop runs.
#[test]
fn bad_rep_code_is_rejected() {
    let mut data = read("tiny.p9e.lzma");
    data[13] = 0x00; // RC_INIT byte must be zero
    data[14] = 0xC0;
    data[15] = 0x00;
    data[16] = 0x00;
    data[17] = 0x00;
    assert!(decode_lzma(&data, usize::MAX, 4096).is_err());
}

/// C: `if (p->tempBufSize != 0 && p->tempBuf[0] != 0) return SZ_ERROR_DATA;`
#[test]
fn nonzero_first_range_coder_byte_is_rejected() {
    let mut data = read("tiny.p9e.lzma");
    data[13] = 0x01;
    assert!(decode_lzma(&data, usize::MAX, 4096).is_err());
    assert!(decode_lzma(&data, 1, 1).is_err());
}

#[test]
fn unsupported_props_are_rejected() {
    let mut data = read("tiny.p9e.lzma");
    data[0] = 9 * 5 * 5;
    assert!(decode_lzma(&data, usize::MAX, 4096).is_err());
}
