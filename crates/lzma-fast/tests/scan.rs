//! The public run scanner must find exactly the boundaries the streams were
//! built with, and must not care how the bytes are handed to it.

mod common;

use lzma_fast::{Error, Lzma2Run, Lzma2RunScanner};

use common::{copy_run, join_runs, multi_run, pseudo_random, xz_run};

/// Scans a whole stream, feeding it `chunk` bytes at a time.
fn scan(data: &[u8], chunk: usize) -> Result<Vec<Lzma2Run>, Error> {
    let mut s = Lzma2RunScanner::new();
    let mut runs = Vec::new();
    let mut pos = 0;
    while pos < data.len() && !s.finished() {
        let end = (pos + chunk).min(data.len());
        pos += s.feed(&data[pos..end])?;
        while let Some(r) = s.next_run() {
            runs.push(r);
        }
    }
    while let Some(r) = s.next_run() {
        runs.push(r);
    }
    assert!(s.finished(), "scanner did not reach the end marker");
    Ok(runs)
}

#[test]
fn finds_every_run_in_a_compressed_stream() {
    let names = ["text.p1.xz", "mixed.p1.xz", "rand.p1.xz", "zeros.p1.xz"];
    let (_, packed, plain) = multi_run(&names, 3);

    let runs = scan(&packed, usize::MAX).expect("scan");
    assert_eq!(runs.len(), names.len() * 3);

    let mut in_pos = 0u64;
    let mut out_pos = 0u64;
    for r in &runs {
        assert!(r.has_dict_reset);
        assert_eq!(r.in_offset, in_pos);
        assert_eq!(r.out_offset, out_pos);
        in_pos += r.packed_len;
        out_pos += r.unpacked_len;
    }
    // Everything but the one-byte end marker is accounted for.
    assert_eq!(in_pos, packed.len() as u64 - 1);
    assert_eq!(out_pos, plain.len() as u64);
}

#[test]
fn a_split_header_is_carried_over_not_re_read() {
    let (_, packed, _) = multi_run(&["text.p1.xz", "mixed.p1.xz"], 2);
    let whole = scan(&packed, usize::MAX).expect("scan");
    // One byte at a time splits every multi-byte chunk header there is.
    for chunk in [1usize, 2, 3, 5, 7, 13, 64, 1021] {
        assert_eq!(scan(&packed, chunk).expect("scan"), whole, "chunk={chunk}");
    }
}

#[test]
fn uncompressed_chunks_delimit_runs_too() {
    // 0x01 resets the dictionary and so starts a run; 0x02 does not.
    let runs = [
        copy_run(&pseudo_random(200_000, 1)),
        copy_run(&pseudo_random(3, 2)),
        copy_run(&pseudo_random(70_000, 3)),
    ];
    let (packed, plain) = join_runs(&runs);
    let found = scan(&packed, 3).expect("scan");
    assert_eq!(found.len(), 3);
    assert_eq!(found[0].unpacked_len, 200_000);
    assert_eq!(found[1].unpacked_len, 3);
    assert_eq!(found[2].unpacked_len, 70_000);
    assert_eq!(
        found.iter().map(|r| r.unpacked_len).sum::<u64>(),
        plain.len() as u64
    );
}

#[test]
fn a_compressed_run_followed_by_a_copy_run_sizes_both() {
    // Regression: the chunk-header state machine ORs into `unpackSize`, so a
    // walk that skips a compressed payload must zero it the way the decoder's
    // own countdown does, or the next copy chunk inherits its high bits.
    let (_, lzma) = xz_run("text.p1.xz");
    let plain_len = lzma.plain.len() as u64;
    let copy = copy_run(&pseudo_random(1000, 9));
    let (packed, _) = join_runs(&[lzma, copy]);
    let found = scan(&packed, usize::MAX).expect("scan");
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].unpacked_len, plain_len);
    assert_eq!(found[1].unpacked_len, 1000);
}

#[test]
fn a_stream_that_never_resets_the_dictionary_is_one_run() {
    // What `7zz -mmt=1` produces: one reset at the start and nothing after it.
    // There is no second boundary to cut at, so there is nothing to decode in
    // parallel, and the multi-threaded decoder has to fall back to the
    // single-threaded one for the whole stream.
    let (_, packed, _) = multi_run(&["text.p1.xz"], 1);
    let found = scan(&packed, 7).expect("scan");
    assert_eq!(found.len(), 1);
    assert!(found[0].has_dict_reset);

    // The format has no way to express a run without one: `needInitLevel`
    // starts at 0xE0, so the first chunk must reset the dictionary.
    let mut broken = packed.clone();
    broken[0] &= 0xDF;
    let mut s = Lzma2RunScanner::new();
    assert_eq!(s.feed(&broken), Err(Error::CorruptData));
}

#[test]
fn a_bad_control_byte_is_an_error() {
    let mut s = Lzma2RunScanner::new();
    // 0x7F is neither a copy chunk (<= 2) nor an LZMA chunk (>= 0x80).
    assert_eq!(s.feed(&[0x7F]), Err(Error::CorruptData));
    // And the error sticks.
    assert_eq!(s.feed(&[0xE0]), Err(Error::CorruptData));
}

#[test]
fn an_lzma_chunk_before_any_dictionary_reset_is_an_error() {
    let mut s = Lzma2RunScanner::new();
    assert_eq!(s.feed(&[0x80]), Err(Error::CorruptData));
}

#[test]
fn nothing_past_the_end_marker_is_consumed() {
    let (_, r) = xz_run("tiny.p1.xz");
    let (mut packed, _) = join_runs(&[r]);
    let len = packed.len();
    packed.extend_from_slice(b"trailing garbage");
    let mut s = Lzma2RunScanner::new();
    assert_eq!(s.feed(&packed), Ok(len));
    assert!(s.finished());
    assert_eq!(s.in_position(), len as u64);
}
