//! The multi-threaded LZMA2 decoder: it must agree with the single-threaded
//! one, byte for byte, whatever the thread count and however the input is fed.

mod common;

use lzma_fast::{Lzma2MtOptions, Lzma2ParallelDecoder};

use common::multi_run;

const THREADS: &[usize] = &[1, 2, 3, 8, 16];

fn opts(threads: usize) -> Lzma2MtOptions {
    Lzma2MtOptions {
        threads,
        memory_limit: u64::MAX,
    }
}

fn decode_mt(dict_prop: u8, packed: &[u8], threads: usize) -> std::io::Result<Vec<u8>> {
    let dec = Lzma2ParallelDecoder::new(dict_prop, &opts(threads)).expect("props");
    let mut out = Vec::new();
    dec.decode(packed, &mut out)?;
    Ok(out)
}

#[test]
fn mt_matches_st_on_a_multi_run_stream() {
    let (prop, packed, plain) = multi_run(&["text.p1.xz", "mixed.p1.xz", "rand.p1.xz"], 4);
    for &t in THREADS {
        let got = decode_mt(prop, &packed, t).unwrap_or_else(|e| panic!("threads={t}: {e}"));
        assert_eq!(got, plain, "threads={t}");
    }
}

#[test]
fn mt_matches_st_on_a_single_run_stream() {
    let (prop, packed, plain) = multi_run(&["text.p1.xz"], 1);
    for &t in THREADS {
        let got = decode_mt(prop, &packed, t).unwrap_or_else(|e| panic!("threads={t}: {e}"));
        assert_eq!(got, plain, "threads={t}");
    }
}
