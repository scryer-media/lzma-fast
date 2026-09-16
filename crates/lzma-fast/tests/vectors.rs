//! Differential correctness against committed vectors.
//!
//! Every vector was produced by `xz(1)` from the `src_*.bin` file with the
//! same stem, so "decodes to the source bytes" is the same statement as
//! "agrees with `xz -dc`".

mod common;

#[cfg(feature = "std")]
use std::io::Read;

use common::*;
use lzma_fast::{Error, FinishMode, LzmaDecoder, LzmaProps, Status};

/// Input/output chunk pairs. The small ones force the `tempBuf` and
/// `LzmaDec_TryDummy` paths on nearly every symbol; the large ones exercise
/// the fast loop's own limit checks.
const CHUNKS: &[(usize, usize)] = &[
    (1, 1),
    (1, 4096),
    (7, 13),
    (4096, 1),
    (4096, 4096),
    (usize::MAX, 1 << 20),
    (1 << 20, 1 << 20),
];

#[test]
fn lzma1_vectors_match_source() {
    for stem in SOURCES {
        let expected = read(&format!("src_{stem}.bin"));
        for variant in LZMA_VARIANTS {
            let name = format!("{stem}.{variant}.lzma");
            let data = read(&name);
            for &(in_chunk, out_chunk) in CHUNKS {
                let got = decode_lzma(&data, in_chunk, out_chunk)
                    .unwrap_or_else(|e| panic!("{name} ({in_chunk},{out_chunk}): {e}"));
                assert_eq!(
                    got.bytes, expected,
                    "{name} ({in_chunk},{out_chunk}) bytes differ"
                );
                if !expected.is_empty() {
                    assert_eq!(
                        got.status,
                        Status::FinishedWithMark,
                        "{name} ({in_chunk},{out_chunk}) status"
                    );
                }
            }
        }
    }
}

#[test]
fn lzma2_vectors_match_source() {
    for stem in SOURCES {
        let expected = read(&format!("src_{stem}.bin"));
        for variant in XZ_VARIANTS {
            let name = format!("{stem}.{variant}.xz");
            let data = read(&name);
            // An xz stream over empty input holds no block at all.
            let Some((dict_prop, payload)) = xz_lzma2_block(&data) else {
                assert!(expected.is_empty(), "{name}: not a single LZMA2 block");
                continue;
            };
            for &(in_chunk, out_chunk) in CHUNKS {
                let got = decode_lzma2(dict_prop, payload, in_chunk, out_chunk)
                    .unwrap_or_else(|e| panic!("{name} ({in_chunk},{out_chunk}): {e}"));
                assert_eq!(
                    got.bytes, expected,
                    "{name} ({in_chunk},{out_chunk}) bytes differ"
                );
            }
        }
    }
}

/// `xz` always writes the unknown-size sentinel, so the committed vectors never
/// exercise `LZMA_FINISH_END` with a known size. Patching the header's size
/// field makes the same stream drive the end-marker lookahead in
/// `LzmaDec_TryDummy`.
#[test]
fn lzma1_with_known_size_finishes_in_end_mode() {
    for stem in SOURCES {
        let expected = read(&format!("src_{stem}.bin"));
        for variant in LZMA_VARIANTS {
            let name = format!("{stem}.{variant}.lzma");
            let mut data = read(&name);
            data[5..13].copy_from_slice(&(expected.len() as u64).to_le_bytes());
            for &(in_chunk, out_chunk) in CHUNKS {
                let got = decode_lzma(&data, in_chunk, out_chunk)
                    .unwrap_or_else(|e| panic!("{name} sized ({in_chunk},{out_chunk}): {e}"));
                assert_eq!(got.bytes, expected, "{name} sized bytes differ");
            }
        }
    }
}

/// The reader adapters are `std`-only, so this only exists when `std` is on.
#[cfg(feature = "std")]
#[test]
fn reader_adapters_round_trip() {
    for stem in SOURCES {
        let expected = read(&format!("src_{stem}.bin"));

        let data = read(&format!("{stem}.p9e.lzma"));
        let mut out = Vec::new();
        lzma_fast::LzmaReader::new(std::io::Cursor::new(&data))
            .unwrap()
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(out, expected, "LzmaReader {stem}");

        let data = read(&format!("{stem}.p1.xz"));
        if let Some((dict_prop, payload)) = xz_lzma2_block(&data) {
            let mut out = Vec::new();
            lzma_fast::Lzma2Reader::new(std::io::Cursor::new(payload), dict_prop)
                .unwrap()
                .read_to_end(&mut out)
                .unwrap();
            assert_eq!(out, expected, "Lzma2Reader {stem}");
        } else {
            assert!(
                expected.is_empty(),
                "{stem}.p1.xz: not a single LZMA2 block"
            );
        }
    }
}

#[test]
fn reset_restarts_a_stream() {
    let expected = read("src_text.bin");
    let data = read("text.p9e.lzma");
    let header: [u8; 13] = data[..13].try_into().unwrap();
    let props = lzma_fast::LzmaAloneHeader::parse(&header).unwrap().props;
    let mut dec = LzmaDecoder::new(props).unwrap();

    for _ in 0..2 {
        let mut out = vec![0u8; expected.len() + 16];
        let p = dec
            .decode(&data[13..], &mut out, FinishMode::Any)
            .expect("decode");
        assert_eq!(&out[..p.written], &expected[..]);
        assert_eq!(p.status, Status::FinishedWithMark);
        dec.reset();
    }
}

#[test]
fn props_parse_matches_reference_arithmetic() {
    // C: LzmaProps_Decode. d = (pb * 5 + lp) * 9 + lc.
    for lc in 0u8..9 {
        for lp in 0u8..5 {
            for pb in 0u8..5 {
                let d = (pb * 5 + lp) * 9 + lc;
                let props = [d, 0x00, 0x00, 0x01, 0x00];
                let parsed = LzmaProps::parse(&props).expect("valid props");
                assert_eq!((parsed.lc(), parsed.lp(), parsed.pb()), (lc, lp, pb));
                assert_eq!(parsed.dict_size(), 0x0001_0000);
            }
        }
    }
    assert_eq!(
        LzmaProps::parse(&[9 * 5 * 5, 0, 0, 1, 0]).unwrap_err(),
        Error::UnsupportedProps
    );
    // C: dicSize below LZMA_DIC_MIN is raised to it.
    assert_eq!(
        LzmaProps::parse(&[0x5D, 1, 0, 0, 0]).unwrap().dict_size(),
        1 << 12
    );
}

#[test]
fn lzma2_rejects_bad_dict_prop() {
    assert!(lzma_fast::Lzma2Decoder::new(41).is_err());
    assert!(lzma_fast::Lzma2Decoder::new(0).is_ok());
}
