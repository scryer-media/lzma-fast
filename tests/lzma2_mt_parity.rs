//! The multi-threaded LZMA2 encoder: bit-exactness against the SDK built
//! *without* `Z7_ST`, which is the only build that reaches `MtCoder.c` and
//! `LzFindMt.c`.
//!
//! The reference side needs `cargo xtask lzma-util`, which builds
//! `lzma2-oracle-mt` beside the single-threaded oracles, and skips with a
//! message when it is missing unless `LZMA_TURBO_LZMA_UTIL_REQUIRE` is set.
//!
//! Three things are checked, at several block sizes and thread counts:
//!
//! * the bytes are what the C produces for the same block size and thread
//!   count;
//! * they do not depend on the thread count, so one thread and eight give the
//!   same stream;
//! * they still decode back to the input through this crate's own reader.

#![cfg(all(feature = "std", feature = "enc"))]

mod corpus;

use std::{io::Read, process::Command};

use corpus::{corpus, tempdir, tool};
use lzma_turbo::{Lzma2Encoder, Lzma2Reader, LzmaEncProps, MatchFinderKind};

/// The arguments `lzma2-oracle-mt` takes, in its order: the LZMA settings, the
/// block size (0 for solid), the block thread count, and the match finder's
/// thread count.
fn oracle_args(
    props: &LzmaEncProps,
    block_size: u64,
    block_threads: usize,
    mf_threads: usize,
) -> Vec<String> {
    let n = props.normalized();
    let mut args: Vec<String> = [
        n.level,
        n.bt_mode,
        n.num_hash_bytes,
        n.lc,
        n.lp,
        n.pb,
        n.fb,
        n.dict_size,
    ]
    .iter()
    .map(u32::to_string)
    .collect();
    args.push(block_size.to_string());
    args.push(block_threads.to_string());
    args.push(mf_threads.to_string());
    args
}

fn settings() -> Vec<LzmaEncProps> {
    vec![
        LzmaEncProps::new().with_level(1).with_dict_size(1 << 16),
        LzmaEncProps::new().with_level(5).with_dict_size(1 << 16),
        LzmaEncProps::new().with_level(9).with_dict_size(1 << 18),
        LzmaEncProps::new()
            .with_level(6)
            .with_match_finder(MatchFinderKind::Hc4)
            .with_dict_size(1 << 16),
        LzmaEncProps::new()
            .with_level(6)
            .with_match_finder(MatchFinderKind::Bt2)
            .with_dict_size(1 << 16),
        LzmaEncProps::new()
            .with_level(5)
            .with_lclppb(0, 2, 0)
            .with_dict_size(1 << 16),
    ]
}

/// The block sizes to split at. They sit either side of the corpus's sizes so
/// that some cases are one block, some several, and some an exact multiple.
const BLOCK_SIZES: [u64; 4] = [1 << 14, 1 << 16, 100_000, 1 << 20];

/// What this crate produces for one `(props, block size, threads)`.
fn ours(props: &LzmaEncProps, src: &[u8], block_size: u64, threads: usize) -> (u8, Vec<u8>) {
    ours_mf(props, src, block_size, threads, 1)
}

/// The same, with the match finder's own thread count, which is what turns on
/// the threaded match finder. C: `lzmaProps.numThreads`.
fn ours_mf(
    props: &LzmaEncProps,
    src: &[u8],
    block_size: u64,
    threads: usize,
    mf_threads: u32,
) -> (u8, Vec<u8>) {
    let mut enc = Lzma2Encoder::new(&props.with_num_threads(mf_threads)).expect("encoder");
    enc.set_block_size(block_size);
    enc.set_threads(threads);
    let out = enc.encode_to_vec(src).expect("encode");
    (enc.properties(), out)
}

#[test]
fn block_parallel_lzma2_matches_the_multi_threaded_reference() {
    let Some(oracle) = tool("lzma2-oracle-mt") else {
        return;
    };
    let dir = tempdir("lzma2-mt-parity");
    let src_path = dir.join("in.bin");
    let ref_path = dir.join("ref.lzma2");

    let settings = settings();
    let mut compared = 0usize;
    for (name, src) in corpus() {
        std::fs::write(&src_path, &src).unwrap();
        for props in &settings {
            for &block_size in &BLOCK_SIZES {
                for threads in [1usize, 2, 4] {
                    let status = Command::new(&oracle)
                        .args(oracle_args(props, block_size, threads, 1))
                        .arg(&src_path)
                        .arg(&ref_path)
                        .status()
                        .expect("run the multi-threaded reference LZMA2 encoder");
                    assert!(status.success(), "reference failed on {name}");

                    let (prop, out) = ours(props, &src, block_size, threads);
                    let mut got = vec![prop];
                    got.extend_from_slice(&out);

                    let expected = std::fs::read(&ref_path).unwrap();
                    assert!(
                        expected == got,
                        "not bit-exact on {name}, block {block_size}, {threads} threads: \
                         reference {} bytes, ours {} bytes",
                        expected.len(),
                        got.len()
                    );
                    compared += 1;
                }
            }
        }
    }
    assert!(compared > 0);
    eprintln!("compared {compared} threaded LZMA2 streams against the reference");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same, with the match finder threaded as well: C `numTotalThreads` is
/// `numBlockThreads_Max * lzmaProps.numThreads`, and the two are independent.
#[test]
fn the_threaded_match_finder_matches_the_multi_threaded_reference() {
    let Some(oracle) = tool("lzma2-oracle-mt") else {
        return;
    };
    let dir = tempdir("lzma2-mtmf-parity");
    let src_path = dir.join("in.bin");
    let ref_path = dir.join("ref.lzma2");

    let settings = settings();
    let mut compared = 0usize;
    for (name, src) in corpus() {
        std::fs::write(&src_path, &src).unwrap();
        for props in &settings {
            for &block_size in &[1u64 << 16, 1 << 20] {
                for threads in [1usize, 4] {
                    let status = Command::new(&oracle)
                        .args(oracle_args(props, block_size, threads, 2))
                        .arg(&src_path)
                        .arg(&ref_path)
                        .status()
                        .expect("run the multi-threaded reference LZMA2 encoder");
                    assert!(status.success(), "reference failed on {name}");

                    let (prop, out) = ours_mf(props, &src, block_size, threads, 2);
                    let mut got = vec![prop];
                    got.extend_from_slice(&out);

                    let expected = std::fs::read(&ref_path).unwrap();
                    assert!(
                        expected == got,
                        "not bit-exact on {name}, block {block_size}, {threads} block threads, \
                         2 match finder threads: reference {} bytes, ours {} bytes",
                        expected.len(),
                        got.len()
                    );
                    compared += 1;
                }
            }
        }
    }
    assert!(compared > 0);
    eprintln!("compared {compared} threaded-match-finder LZMA2 streams against the reference");
    let _ = std::fs::remove_dir_all(&dir);
}

/// C: `Lzma2EncProps_Normalize` divides `numTotalThreads` by the match
/// finder's thread count to get the block thread count, so the same total
/// spent either way gives the same bytes.
#[test]
fn the_total_thread_budget_splits_the_way_the_c_splits_it() {
    let props = LzmaEncProps::new().with_level(5).with_dict_size(1 << 16);
    for (name, src) in corpus() {
        let want = ours(&props, &src, 1 << 16, 1);
        for total in [1usize, 2, 4, 8] {
            let mut enc = Lzma2Encoder::new(&props.with_num_threads(2)).expect("encoder");
            enc.set_block_size(1 << 16);
            enc.set_total_threads(total);
            let out = enc.encode_to_vec(&src).expect("encode");
            assert!(
                (enc.properties(), out) == want,
                "{name}: a total budget of {total} changed the bytes"
            );
        }
    }
}

#[test]
fn the_thread_count_does_not_change_the_bytes() {
    for props in settings() {
        for (name, src) in corpus() {
            for &block_size in &BLOCK_SIZES {
                let want = ours(&props, &src, block_size, 1);
                for threads in [2usize, 3, 8, 17] {
                    let got = ours(&props, &src, block_size, threads);
                    assert!(
                        got == want,
                        "{name}: block {block_size} at {threads} threads differs from one thread"
                    );
                }
            }
        }
    }
}

#[test]
fn threaded_streams_round_trip_through_the_decoder() {
    for props in settings() {
        for (name, src) in corpus() {
            for &block_size in &BLOCK_SIZES {
                let (prop, encoded) = ours(&props, &src, block_size, 4);
                let mut reader =
                    Lzma2Reader::new(std::io::Cursor::new(&encoded), prop).expect("reader");
                let mut out = Vec::new();
                reader.read_to_end(&mut out).expect("decode");
                assert!(
                    out == src,
                    "round trip lost bytes on {name}, block {block_size}"
                );
            }
        }
    }
}
