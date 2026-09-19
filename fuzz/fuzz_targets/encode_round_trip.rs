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
use lzma_turbo::xz::bcj::BcjKind;
use lzma_turbo::xz::{
    CheckType, FILTER_DELTA, FilterFlags, XzAdaptiveDecoder, XzOptions, XzParallelReader, XzReader,
};
use lzma_turbo::{
    DrainStatus, LzmaEncProps, MatchFinderKind, encode_lzma2, encode_lzma2_mt, encode_lzma_alone,
    encode_xz, encode_xz_mt, encode_xz_with_filters,
};
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

        // The same LZMA2 stream again, but cut into blocks and compressed by
        // several threads at once. The block threads must not change what
        // comes out: the bytes have to be exactly the solid-block encoder's
        // for the same block size, whatever the thread count.
        let block_size = (u64::from(cfg[3]) + 1) * 4096;
        let threads = usize::from(cfg[2] % 4) + 1;
        if let Ok((mt_prop, mt_lzma2)) = encode_lzma2_mt(src, &props, block_size, threads) {
            let (one_prop, one_lzma2) =
                encode_lzma2_mt(src, &props, block_size, 1).expect("one thread encodes too");
            assert_eq!(mt_prop, one_prop, "the property byte depends on the threads");
            assert_eq!(mt_lzma2, one_lzma2, "the bytes depend on the threads");

            let mut dec = Lzma2Decoder::new(mt_prop).expect("our own property byte");
            let mut out = Vec::new();
            let mut input = &mt_lzma2[..];
            let mut buf = vec![0u8; 8192];
            loop {
                let p = dec
                    .decode(input, &mut buf, lzma_turbo::FinishMode::Any)
                    .expect("our own threaded LZMA2 decodes");
                out.extend_from_slice(&buf[..p.written]);
                input = &input[p.read..];
                if p.status == lzma_turbo::Status::FinishedWithMark {
                    break;
                }
                assert!(p.read != 0 || p.written != 0, "LZMA2 decode stalled");
            }
            assert_eq!(out, src, "threaded LZMA2 round trip");
        }

        // .xz, over the same settings.
        let check = match cfg[3] % 4 {
            0 => CheckType::None,
            1 => CheckType::Crc32,
            2 => CheckType::Crc64,
            _ => CheckType::Sha256,
        };
        let xz_block_size = u64::from(cfg[3]) * 1024;
        let xz = encode_xz(src, &props, check, xz_block_size).expect("our own .xz encodes");
        let mut out = Vec::new();
        XzReader::with_options(Cursor::new(&xz), XzOptions::default())
            .read_to_end(&mut out)
            .expect("our own .xz decodes");
        assert_eq!(out, src, ".xz round trip");

        // .xz through a filter chain, which the byte after the settings
        // picks: a BCJ converter, delta, or nothing.
        let filters: Vec<FilterFlags> = match cfg[0] % 11 {
            0 => Vec::new(),
            n @ 1..=8 => {
                let kind = [
                    BcjKind::X86,
                    BcjKind::Ppc,
                    BcjKind::Ia64,
                    BcjKind::Arm,
                    BcjKind::ArmThumb,
                    BcjKind::Sparc,
                    BcjKind::Arm64,
                    BcjKind::RiscV,
                ][(n - 1) as usize];
                vec![FilterFlags::new(kind.filter_id(), &[]).expect("no props")]
            }
            9 => vec![FilterFlags::new(FILTER_DELTA, &[cfg[1]]).expect("one prop")],
            _ => vec![
                FilterFlags::new(FILTER_DELTA, &[cfg[1]]).expect("one prop"),
                FilterFlags::new(BcjKind::X86.filter_id(), &[]).expect("no props"),
            ],
        };
        let xz = encode_xz_with_filters(src, &props, check, xz_block_size, &filters)
            .expect("our own filtered .xz encodes");

        // And the same filtered stream written by several block threads, which
        // must be the same bytes again.
        let xz_mt = encode_xz_mt(src, &props, check, xz_block_size, &filters, threads)
            .expect("our own threaded filtered .xz encodes");
        assert_eq!(xz_mt, xz, "the .xz bytes depend on the threads");

        let mut out = Vec::new();
        XzReader::with_options(Cursor::new(&xz), XzOptions::default())
            .read_to_end(&mut out)
            .expect("our own filtered .xz decodes");
        assert_eq!(out, src, "filtered .xz round trip");

        // The other two readers see the same stream differently — the
        // parallel one whole, the adaptive one in pieces — and a converter's
        // carry is exactly what differs between those two paths.
        let mut out = Vec::new();
        XzParallelReader::with_options(Cursor::new(&xz), XzOptions::default().with_threads(2))
            .expect("our own filtered .xz opens")
            .read_to_end(&mut out)
            .expect("our own filtered .xz decodes in parallel");
        assert_eq!(out, src, "filtered .xz parallel round trip");

        let mut dec = XzAdaptiveDecoder::new(XzOptions::default().with_threads(2));
        let mut out: Vec<u8> = Vec::new();
        let mut pos = 0usize;
        let chunk = usize::from(cfg[2]).max(1);
        if xz.is_empty() {
            dec.end_of_input();
        }
        loop {
            if pos < xz.len() {
                pos += dec
                    .feed(&xz[pos..(pos + chunk).min(xz.len())])
                    .expect("our own filtered .xz feeds");
                if pos == xz.len() {
                    dec.end_of_input();
                }
            }
            let status = dec
                .drain(|off, bytes| {
                    let off = usize::try_from(off).expect("offset");
                    if out.len() < off + bytes.len() {
                        out.resize(off + bytes.len(), 0);
                    }
                    out[off..off + bytes.len()].copy_from_slice(bytes);
                })
                .expect("our own filtered .xz drains");
            if status == DrainStatus::Finished {
                break;
            }
            assert!(pos < xz.len() || status != DrainStatus::NeedsMoreInput, "stalled");
        }
        assert_eq!(out, src, "filtered .xz adaptive round trip");
    }
});
