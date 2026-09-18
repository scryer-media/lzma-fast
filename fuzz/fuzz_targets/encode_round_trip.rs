//! Arbitrary bytes through the encoders and back through the decoders.
//!
//! The parity tests prove the encoder agrees with the SDK on a fixed corpus;
//! this asks the weaker question over arbitrary input: whatever it produces,
//! this crate's own decoders must return exactly what went in. The settings
//! come from the input's first bytes, so the fuzzer explores the match
//! finders, the levels and the container options too.
#![no_main]

use std::io::{Cursor, Read};

use libfuzzer_sys::fuzz_target;
use lzma_turbo::xz::{CheckType, XzOptions, XzReader};
use lzma_turbo::{LzmaEncProps, MatchFinderKind, encode_lzma2, encode_lzma_alone, encode_xz};
use lzma_turbo::{Lzma2Decoder, LzmaAloneHeader, LzmaReader};

fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }
    let (cfg, src) = data.split_at(4);

    let finder = match cfg[0] % 6 {
        0 => MatchFinderKind::Hc4,
        1 => MatchFinderKind::Hc5,
        2 => MatchFinderKind::Bt2,
        3 => MatchFinderKind::Bt3,
        4 => MatchFinderKind::Bt4,
        _ => MatchFinderKind::Bt5,
    };
    // A small dictionary keeps the fuzzer's memory flat and is the
    // interesting end anyway: it is where the window moves and normalizes.
    let dict_size = 1u32 << (12 + u32::from(cfg[1] % 8));
    let props = LzmaEncProps::new()
        .with_level(u32::from(cfg[2] % 10))
        .with_dict_size(dict_size)
        .with_match_finder(finder);

    // .lzma (LZMA-Alone).
    if let Ok(alone) = encode_lzma_alone(src, &props) {
        let head: &[u8; 13] = alone[..13].try_into().expect("13 bytes");
        let header = LzmaAloneHeader::parse(head).expect("our own header parses");
        assert_eq!(header.uncompressed_size, Some(src.len() as u64));
        let mut out = Vec::new();
        LzmaReader::new(Cursor::new(&alone))
            .expect("our own .lzma opens")
            .read_to_end(&mut out)
            .expect("our own .lzma decodes");
        assert_eq!(out, src, ".lzma round trip");
    }

    // Raw LZMA2, which needs lc + lp <= 4.
    if let Ok((dict_prop, lzma2)) = encode_lzma2(src, &props) {
        let mut dec = Lzma2Decoder::new(dict_prop).expect("our own property byte");
        let mut out = Vec::new();
        let mut input = &lzma2[..];
        let mut buf = vec![0u8; 8192];
        loop {
            let p = dec
                .decode(input, &mut buf, lzma_turbo::FinishMode::Any)
                .expect("our own LZMA2 decodes");
            out.extend_from_slice(&buf[..p.written]);
            input = &input[p.read..];
            if p.status == lzma_turbo::Status::FinishedWithMark {
                break;
            }
            assert!(p.read != 0 || p.written != 0, "LZMA2 decode stalled");
        }
        assert_eq!(out, src, "LZMA2 round trip");

        // .xz, over the same settings.
        let check = match cfg[3] % 4 {
            0 => CheckType::None,
            1 => CheckType::Crc32,
            2 => CheckType::Crc64,
            _ => CheckType::Sha256,
        };
        let block_size = u64::from(cfg[3]) * 1024;
        let xz = encode_xz(src, &props, check, block_size).expect("our own .xz encodes");
        let mut out = Vec::new();
        XzReader::with_options(Cursor::new(&xz), XzOptions::default())
            .read_to_end(&mut out)
            .expect("our own .xz decodes");
        assert_eq!(out, src, ".xz round trip");
    }
});
