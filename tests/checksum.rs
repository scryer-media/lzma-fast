//! Worker-side checksums: the decoder computes them where the bytes are
//! produced, and the caller folds the pieces into the ranges it cares about.
//!
//! The property under test throughout is the one that makes the feature usable
//! at all: folding the pieces must give exactly what checksumming the whole
//! output in one pass would have given, for every range the split points
//! bound, at every thread count, on both the parallel and the adaptive path.
#![cfg(all(feature = "std", feature = "crc"))]

mod common;

use lzma_turbo::crc::{Crc32, Crc64Xz, CrcFolder, crc32, crc64_xz};
use lzma_turbo::{
    BlockChecks, Checksum, ChecksumPlan, DrainStatus, Lzma2AdaptiveDecoder, Lzma2MtOptions,
    Lzma2ParallelDecoder, Lzma2ParallelReader, Segment, SegmentCheck,
};

use common::multi_run;

const THREADS: &[usize] = &[1, 2, 3, 8, 16];

fn opts(threads: usize) -> Lzma2MtOptions {
    Lzma2MtOptions {
        threads,
        memory_limit: u64::MAX,
    }
}

/// Split points chosen to be awkward: a 1, a prime stride that lands inside
/// runs, and the exact run boundaries of the fixture streams, which is where
/// an off-by-one between "block start" and "segment start" would hide.
fn awkward_splits(len: u64, run_len: u64) -> Vec<u64> {
    let mut v = vec![1u64, 3, 7];
    let mut p = 10_007u64;
    while p < len {
        v.push(p);
        p += 10_007;
    }
    // Every run boundary, plus one byte either side of it.
    let mut b = run_len;
    while b < len {
        v.push(b - 1);
        v.push(b);
        v.push(b + 1);
        b += run_len;
    }
    v.retain(|&x| x < len);
    v.sort_unstable();
    v.dedup();
    v
}

/// Pushes every segment into a folder and checks it against one-shot
/// checksums of the same ranges of `plain`.
fn check_folds(plain: &[u8], checks: &[BlockChecks], splits: &[u64], kind: Checksum, what: &str) {
    let segments: Vec<Segment> = checks
        .iter()
        .flat_map(|c| c.segments.iter().copied())
        .collect();
    assert!(!segments.is_empty(), "{what}: no segments at all");

    // The segments must tile the output exactly, in some order.
    let mut sorted = segments.clone();
    sorted.sort_by_key(|s| s.offset);
    let mut pos = 0u64;
    for s in &sorted {
        assert_eq!(s.offset, pos, "{what}: segments do not tile ({s:?})");
        assert_ne!(s.len, 0, "{what}: empty segment");
        pos += s.len;
    }
    assert_eq!(pos, plain.len() as u64, "{what}: segments stop short");

    // Every boundary the caller asked for must be a segment boundary, or a
    // range query at it could not be answered.
    let bounds: std::collections::BTreeSet<u64> = sorted.iter().map(|s| s.offset).collect();
    for &p in splits {
        assert!(bounds.contains(&p), "{what}: no segment starts at {p}");
    }

    // Fold, in an order deliberately unlike the stream's.
    match kind {
        Checksum::Crc32 => {
            let mut f = CrcFolder::<u32>::new();
            for s in segments.iter().rev() {
                f.push(s.offset, s.len, s.check.crc32().expect("crc32 asked for"));
            }
            assert_eq!(
                f.range(0, plain.len() as u64),
                Some(crc32(plain)),
                "{what}: whole-output fold"
            );
            let mut edges = splits.to_vec();
            edges.insert(0, 0);
            edges.push(plain.len() as u64);
            for w in edges.windows(2) {
                let (a, b) = (w[0], w[1]);
                assert_eq!(
                    f.range(a, b - a),
                    Some(crc32(&plain[a as usize..b as usize])),
                    "{what}: fold of {a}..{b}"
                );
            }
            // A range spanning several split points folds too.
            for w in edges.windows(4) {
                let (a, b) = (w[0], w[3]);
                assert_eq!(
                    f.range(a, b - a),
                    Some(crc32(&plain[a as usize..b as usize])),
                    "{what}: multi-segment fold of {a}..{b}"
                );
            }
        }
        Checksum::Crc64Xz => {
            let mut f = CrcFolder::<u64>::new();
            for s in segments.iter().rev() {
                f.push(s.offset, s.len, s.check.crc64().expect("crc64 asked for"));
            }
            assert_eq!(
                f.range(0, plain.len() as u64),
                Some(crc64_xz(plain)),
                "{what}: whole-output fold"
            );
            let mut edges = splits.to_vec();
            edges.insert(0, 0);
            edges.push(plain.len() as u64);
            for w in edges.windows(2) {
                let (a, b) = (w[0], w[1]);
                assert_eq!(
                    f.range(a, b - a),
                    Some(crc64_xz(&plain[a as usize..b as usize])),
                    "{what}: fold of {a}..{b}"
                );
            }
        }
        _ => panic!("{what}: not a CRC"),
    }
}

/// The blocks must tile the output too, whatever produced them.
fn check_blocks_tile(plain: &[u8], checks: &[BlockChecks], what: &str) {
    let mut sorted: Vec<(u64, u64)> = checks.iter().map(|c| (c.unpacked_offset, c.len)).collect();
    sorted.sort_unstable();
    let mut pos = 0u64;
    for (off, len) in sorted {
        assert_eq!(off, pos, "{what}: blocks do not tile");
        pos += len;
    }
    assert_eq!(pos, plain.len() as u64, "{what}: blocks stop short");
}

// ---------------------------------------------------------------------------
// The parallel decoder
// ---------------------------------------------------------------------------

#[test]
fn parallel_segments_fold_to_the_whole_at_every_thread_count() {
    let (prop, packed, plain) = multi_run(&["text.p1.xz", "mixed.p1.xz", "rand.p1.xz"], 4);
    let run_len = (plain.len() / 12).max(1) as u64;
    let splits = awkward_splits(plain.len() as u64, run_len);

    for kind in [Checksum::Crc32, Checksum::Crc64Xz] {
        let plan = ChecksumPlan::new(kind).with_split_points(splits.iter().copied());
        for &t in THREADS {
            let dec = Lzma2ParallelDecoder::new(prop, &opts(t)).expect("props");
            let mut out = Vec::new();
            let (n, checks) = dec
                .decode_checksummed(&packed[..], &mut out, &plan)
                .unwrap_or_else(|e| panic!("threads={t}: {e}"));
            assert_eq!(n as usize, plain.len());
            assert_eq!(out, plain, "threads={t}: bytes changed");
            let what = format!("{kind:?} threads={t}");
            check_blocks_tile(&plain, &checks, &what);
            check_folds(&plain, &checks, &splits, kind, &what);
        }
    }
}

/// A stream with a single run has no threaded pass at all: it falls through to
/// the single-threaded path, which must still produce segments covering every
/// byte. This is the case a chasing consumer meets most.
#[test]
fn the_single_threaded_fallback_still_segments() {
    let (prop, packed, plain) = multi_run(&["text.p1.xz"], 1);
    let splits = awkward_splits(plain.len() as u64, plain.len() as u64 + 1);
    let plan = ChecksumPlan::new(Checksum::Crc32).with_split_points(splits.iter().copied());
    for &t in THREADS {
        let dec = Lzma2ParallelDecoder::new(prop, &opts(t)).expect("props");
        let mut out = Vec::new();
        let (_, checks) = dec
            .decode_checksummed(&packed[..], &mut out, &plan)
            .unwrap_or_else(|e| panic!("threads={t}: {e}"));
        assert_eq!(out, plain);
        check_folds(
            &plain,
            &checks,
            &splits,
            Checksum::Crc32,
            &format!("st t={t}"),
        );
    }
}

/// With no split points there is one segment per block and folding them gives
/// the whole.
#[test]
fn no_split_points_means_one_segment_per_block() {
    let (prop, packed, plain) = multi_run(&["text.p1.xz", "mixed.p1.xz"], 3);
    let plan = ChecksumPlan::new(Checksum::Crc32);
    let dec = Lzma2ParallelDecoder::new(prop, &opts(4)).expect("props");
    let mut out = Vec::new();
    let (_, checks) = dec
        .decode_checksummed(&packed[..], &mut out, &plan)
        .expect("decode");
    for c in &checks {
        assert_eq!(c.segments.len(), 1, "one segment per block: {c:?}");
        assert_eq!(c.segments[0].offset, c.unpacked_offset);
        assert_eq!(c.segments[0].len, c.len);
    }
    check_blocks_tile(&plain, &checks, "no splits");
    check_folds(&plain, &checks, &[], Checksum::Crc32, "no splits");
}

/// Asking for no checksum must change nothing, including the bytes.
#[test]
fn checksum_none_computes_nothing() {
    let (prop, packed, plain) = multi_run(&["text.p1.xz", "mixed.p1.xz"], 3);
    let dec = Lzma2ParallelDecoder::new(prop, &opts(4)).expect("props");
    let mut out = Vec::new();
    let (_, checks) = dec
        .decode_checksummed(&packed[..], &mut out, &ChecksumPlan::none())
        .expect("decode");
    assert_eq!(out, plain);
    assert!(checks.is_empty(), "{checks:?}");
}

// ---------------------------------------------------------------------------
// The reader adapter
// ---------------------------------------------------------------------------

#[test]
fn the_reader_drains_segments_as_it_goes() {
    use std::io::Read;

    let (prop, packed, plain) = multi_run(&["text.p1.xz", "mixed.p1.xz", "rand.p1.xz"], 4);
    let splits = awkward_splits(plain.len() as u64, (plain.len() / 12).max(1) as u64);
    let plan = ChecksumPlan::new(Checksum::Crc32).with_split_points(splits.iter().copied());

    let mut r = Lzma2ParallelReader::with_checksums(
        std::io::Cursor::new(packed.clone()),
        prop,
        &opts(4),
        &plan,
    )
    .expect("reader");
    let mut got = Vec::new();
    let mut checks = Vec::new();
    let mut buf = [0u8; 7777];
    loop {
        // Drained mid-stream, which is the point of the API.
        checks.extend(r.take_checks());
        let n = r.read(&mut buf).expect("read");
        if n == 0 {
            break;
        }
        got.extend_from_slice(&buf[..n]);
    }
    checks.extend(r.take_checks());
    drop(r);

    assert_eq!(got, plain);
    check_blocks_tile(&plain, &checks, "reader");
    check_folds(&plain, &checks, &splits, Checksum::Crc32, "reader");
}

// ---------------------------------------------------------------------------
// The adaptive decoder
// ---------------------------------------------------------------------------

fn adaptive_checks(
    prop: u8,
    packed: &[u8],
    threads: usize,
    ordered: bool,
    plan: &ChecksumPlan,
    feed: usize,
) -> (Vec<u8>, Vec<BlockChecks>) {
    let mut dec = Lzma2AdaptiveDecoder::new(prop, &opts(threads)).expect("props");
    dec.set_ordered(ordered);
    dec.set_checksum(plan);

    let mut out: Vec<u8> = Vec::new();
    let mut checks = Vec::new();
    let mut at = 0usize;
    loop {
        if at < packed.len() {
            let end = (at + feed).min(packed.len());
            at += dec.feed(&packed[at..end]).expect("feed");
            if at == packed.len() {
                dec.end_of_input();
            }
        }
        let status = dec
            .drain(|off, bytes| {
                let off = off as usize;
                if out.len() < off + bytes.len() {
                    out.resize(off + bytes.len(), 0);
                }
                out[off..off + bytes.len()].copy_from_slice(bytes);
            })
            .expect("drain");
        checks.extend(dec.take_checks());
        if status == DrainStatus::Finished {
            break;
        }
    }
    checks.extend(dec.take_checks());
    (out, checks)
}

#[test]
fn adaptive_segments_fold_to_the_whole() {
    let (prop, packed, plain) = multi_run(&["text.p1.xz", "mixed.p1.xz", "rand.p1.xz"], 4);
    let splits = awkward_splits(plain.len() as u64, (plain.len() / 12).max(1) as u64);

    for kind in [Checksum::Crc32, Checksum::Crc64Xz] {
        let plan = ChecksumPlan::new(kind).with_split_points(splits.iter().copied());
        for &t in THREADS {
            for ordered in [true, false] {
                for feed in [1usize, 997, packed.len()] {
                    let (got, checks) = adaptive_checks(prop, &packed, t, ordered, &plan, feed);
                    assert_eq!(got, plain, "{kind:?} t={t} ordered={ordered} feed={feed}");
                    let what = format!("adaptive {kind:?} t={t} ordered={ordered} feed={feed}");
                    check_blocks_tile(&plain, &checks, &what);
                    check_folds(&plain, &checks, &splits, kind, &what);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SHA-256
// ---------------------------------------------------------------------------

/// SHA-256 is per block and ignores split points, because it cannot be folded.
/// Each block's digest must equal a one-shot digest of that block's bytes.
#[cfg(any(feature = "crypto", feature = "native-crypto"))]
#[test]
fn sha256_is_per_block_and_ignores_split_points() {
    use lzma_turbo::crypto::Sha256;

    let (prop, packed, plain) = multi_run(&["text.p1.xz", "mixed.p1.xz", "rand.p1.xz"], 4);
    let splits = awkward_splits(plain.len() as u64, (plain.len() / 12).max(1) as u64);
    let plan = ChecksumPlan::new(Checksum::Sha256).with_split_points(splits.iter().copied());

    for &t in THREADS {
        let dec = Lzma2ParallelDecoder::new(prop, &opts(t)).expect("props");
        let mut out = Vec::new();
        let (_, checks) = dec
            .decode_checksummed(&packed[..], &mut out, &plan)
            .unwrap_or_else(|e| panic!("threads={t}: {e}"));
        assert_eq!(out, plain, "threads={t}");
        check_blocks_tile(&plain, &checks, &format!("sha t={t}"));
        for c in &checks {
            assert!(c.segments.is_empty(), "sha256 must not segment: {c:?}");
            let a = c.unpacked_offset as usize;
            let b = a + c.len as usize;
            let mut h = Sha256::new();
            h.update(&plain[a..b]);
            assert_eq!(
                c.digest.expect("a digest was asked for"),
                h.finalize(),
                "threads={t} block at {a}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The streaming segmenter against one-shot values
// ---------------------------------------------------------------------------

/// However the bytes arrive - a whole block at once on a worker, 1 MiB at a
/// time on the single-threaded path - the folded answers must be identical.
///
/// The segment *lists* are not identical, and cannot be: a worker only ever
/// sees its own block, so the threaded path always cuts at block boundaries as
/// well as at the caller's split points. That is exactly what the folder is
/// for. What must hold is that the extra cuts are a refinement - every split
/// point is still a boundary on both paths - and that folding either list
/// gives the same checksum for every range the caller can ask about.
#[test]
fn streaming_and_one_shot_agree() {
    let (prop, packed, plain) = multi_run(&["text.p1.xz", "mixed.p1.xz"], 3);
    let splits = awkward_splits(plain.len() as u64, (plain.len() / 6).max(1) as u64);
    let plan = ChecksumPlan::new(Checksum::Crc32).with_split_points(splits.iter().copied());

    let decode = |threads: usize| {
        let dec = Lzma2ParallelDecoder::new(prop, &opts(threads)).expect("props");
        let mut out = Vec::new();
        let (_, checks) = dec
            .decode_checksummed(&packed[..], &mut out, &plan)
            .expect("decode");
        assert_eq!(out, plain, "threads={threads}");
        checks
    };
    let st = decode(1);
    let mt = decode(8);

    // The threaded path cuts more finely, never more coarsely.
    let flat = |v: &[BlockChecks]| -> Vec<Segment> {
        let mut s: Vec<Segment> = v.iter().flat_map(|c| c.segments.iter().copied()).collect();
        s.sort_by_key(|x| x.offset);
        s
    };
    let (st_flat, mt_flat) = (flat(&st), flat(&mt));
    assert!(
        mt_flat.len() > st_flat.len(),
        "the threaded path should cut at its block boundaries too"
    );
    let mt_bounds: std::collections::BTreeSet<u64> = mt_flat.iter().map(|s| s.offset).collect();
    for s in &st_flat {
        assert!(mt_bounds.contains(&s.offset), "not a refinement at {s:?}");
    }

    // Each segment on each path is a one-shot checksum of exactly its bytes.
    for s in st_flat.iter().chain(mt_flat.iter()) {
        let a = s.offset as usize;
        let b = a + s.len as usize;
        assert_eq!(s.check, SegmentCheck::Crc32(crc32(&plain[a..b])), "{s:?}");
    }

    // And both fold to the same answers for every range the caller can ask
    // about, which is the only thing a consumer observes.
    let fold = |segs: &[Segment]| {
        let mut f = CrcFolder::<u32>::new();
        for s in segs {
            f.push(s.offset, s.len, s.check.crc32().expect("crc32"));
        }
        f
    };
    let (fst, fmt) = (fold(&st_flat), fold(&mt_flat));
    let mut edges = splits.clone();
    edges.insert(0, 0);
    edges.push(plain.len() as u64);
    for i in 0..edges.len() {
        for j in i + 1..edges.len() {
            let (a, b) = (edges[i], edges[j]);
            let want = crc32(&plain[a as usize..b as usize]);
            assert_eq!(fst.range(a, b - a), Some(want), "st fold {a}..{b}");
            assert_eq!(fmt.range(a, b - a), Some(want), "mt fold {a}..{b}");
        }
    }
}

/// The two digest types the crate exposes stay usable directly; a segment's
/// checksum is the same thing a caller would compute by hand.
#[test]
fn a_segment_is_just_a_checksum() {
    let data = b"the quick brown fox jumps over the lazy dog";
    let mut a = Crc32::new();
    a.update(data);
    assert_eq!(SegmentCheck::Crc32(a.finalize()).crc32(), Some(crc32(data)));
    let mut b = Crc64Xz::new();
    b.update(data);
    assert_eq!(
        SegmentCheck::Crc64(b.finalize()).crc64(),
        Some(crc64_xz(data))
    );
}
