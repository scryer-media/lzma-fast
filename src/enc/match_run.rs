//! The match-extension scan the binary-tree and hash-chain finders run.
//!
//! C: the byte loops inside `UPDATE_maxLen`, `GetMatchesSpec1`,
//! `SkipMatchesSpec`, `Hc_GetMatchesSpec` (`C/LzFind.c`) and
//! `GetMatchesSpecN_2` (`C/LzFindOpt.c`). Every one of them is the same
//! question asked with different index arithmetic: starting at some position
//! in the window, how far do the bytes at `i` and the bytes `diff` behind them
//! agree, stopping at a limit? The SDK asks it one byte at a time. This asks it
//! eight bytes at a time, which is the same answer: the first differing byte in
//! a 64-bit XOR is the one `trailing_zeros` names once both words are read
//! little-endian, so the byte index is the same on a big-endian host as on a
//! little-endian one.
//!
//! Eight and not sixteen. A sixteen-byte scan is faster at the scan alone -
//! 2.98x the byte loop against 2.46x on an i5-1240P, 3.40x against 2.68x on an
//! M5 Max - and slower at the encode it is part of: 1.2% slower at preset 6 and
//! 0.7% at preset 9, over seven interleaved rounds. The runs the finder walks
//! are mostly shorter than one sixteen-byte word, so what the wider load wins
//! on the long ones does not pay for its tail on the short ones. Both arms were
//! measured end to end and the numbers are in `docs/simd-kernels-report.md`.
//!
//! The scan reads strictly inside `start .. limit` and `start - diff ..
//! limit - diff`, which is a subset of what the byte loop's callers already
//! index: every one of them compares at `limit` itself once the scan returns.
//! So there is no margin to establish and no `unsafe` here.

/// The first index `i` in `start ..= limit` at which `buf[i - diff]` and
/// `buf[i]` differ, or `limit` if they agree the whole way.
///
/// `diff` is the match distance, so `i - diff` is the older copy. The caller
/// guarantees `diff <= start` and `limit <= buf.len()`; `start > limit` is
/// answered with `start` rather than a panic, because the callers reach that
/// only on a path the C never takes.
#[inline(always)]
pub(crate) fn match_run(buf: &[u8], diff: usize, start: usize, limit: usize) -> usize {
    if start >= limit {
        return start;
    }
    #[cfg(feature = "kernel-ab")]
    match crate::kernel_ab::match_run_arm() {
        crate::kernel_ab::MatchRunArm::Scalar => return scalar_run(buf, diff, start, limit),
        crate::kernel_ab::MatchRunArm::Word16 => return run16(buf, diff, start, limit),
        crate::kernel_ab::MatchRunArm::Word8 => {}
    }
    run8(buf, diff, start, limit)
}

/// The eight-byte scan. Both windows are cut once, and the word steps walk
/// them as `chunks_exact` pairs rather than by index: that is what leaves the
/// hot loop with two loads, a compare and a branch and nothing else. Indexing
/// the slices keeps a bounds check in it, because the two lengths are equal by
/// construction and the compiler cannot see that.
#[inline(always)]
fn run8(buf: &[u8], diff: usize, start: usize, limit: usize) -> usize {
    let back = &buf[start - diff..limit - diff];
    let fore = &buf[start..limit];

    let mut off = 0usize;
    for (a, b) in back.chunks_exact(8).zip(fore.chunks_exact(8)) {
        let x =
            u64::from_le_bytes(a.try_into().unwrap()) ^ u64::from_le_bytes(b.try_into().unwrap());
        if x != 0 {
            // `from_le_bytes` puts the byte at the lowest address in the
            // lowest bits on every host, so this is the address order index.
            return start + off + (x.trailing_zeros() as usize >> 3);
        }
        off += 8;
    }
    let n = fore.len();
    while off < n && back[off] == fore[off] {
        off += 1;
    }
    start + off
}

/// The sixteen-byte scan, kept as the other arm of the A/B. It wins a
/// microbenchmark of the scan alone on both hosts and loses the encode it is
/// part of; `docs/simd-kernels-report.md` has the numbers and why.
#[cfg(any(test, feature = "kernel-ab"))]
#[inline(always)]
fn run16(buf: &[u8], diff: usize, start: usize, limit: usize) -> usize {
    let back = &buf[start - diff..limit - diff];
    let fore = &buf[start..limit];

    let mut off = 0usize;
    for (a, b) in back.chunks_exact(16).zip(fore.chunks_exact(16)) {
        let x =
            u128::from_le_bytes(a.try_into().unwrap()) ^ u128::from_le_bytes(b.try_into().unwrap());
        if x != 0 {
            return start + off + (x.trailing_zeros() as usize >> 3);
        }
        off += 16;
    }
    let n = fore.len();
    if off + 8 <= n {
        let x = u64::from_le_bytes(back[off..off + 8].try_into().unwrap())
            ^ u64::from_le_bytes(fore[off..off + 8].try_into().unwrap());
        if x != 0 {
            return start + off + (x.trailing_zeros() as usize >> 3);
        }
        off += 8;
    }
    while off < n && back[off] == fore[off] {
        off += 1;
    }
    start + off
}

/// The SDK's byte loop, kept as the differential reference and as the `b` arm
/// of the A/B measurement.
#[cfg(any(test, feature = "kernel-ab"))]
#[inline(always)]
pub(crate) fn scalar_run(buf: &[u8], diff: usize, start: usize, limit: usize) -> usize {
    let mut i = start;
    while i != limit && buf[i - diff] == buf[i] {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::{match_run, scalar_run};
    use alloc::vec;
    use alloc::vec::Vec;

    /// Every `(diff, start, limit)` the callers can ask for, over a window
    /// whose bytes are chosen so that the answer lands on every offset in a
    /// word and on both sides of every word boundary.
    ///
    /// The scan is only correct if it agrees with the SDK's byte loop for all
    /// of them, so the test is the byte loop.
    fn agree(buf: &[u8]) {
        let n = buf.len();
        for diff in 1..n.min(24) {
            for start in diff..n {
                for limit in start..n {
                    let want = scalar_run(buf, diff, start, limit);
                    let got = match_run(buf, diff, start, limit);
                    assert_eq!(
                        got, want,
                        "diff {diff} start {start} limit {limit} in {buf:?}"
                    );
                    // The wide arm is not what ships, but it is what the A/B
                    // measured against, and a measurement of a wrong answer is
                    // not a measurement.
                    if start < limit {
                        assert_eq!(
                            super::run16(buf, diff, start, limit),
                            want,
                            "run16: diff {diff} start {start} limit {limit}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn matches_the_byte_loop_on_a_run_that_breaks_at_every_offset() {
        // A window that agrees with itself at distance `d` for a while and
        // then does not: sweeping the break point across 0..48 puts the first
        // difference at every position within a word and at every word
        // boundary, which is where a word-at-a-time scan goes wrong.
        for break_at in 0..48usize {
            let mut buf = vec![0xA5u8; 64];
            for (i, b) in buf.iter_mut().enumerate() {
                *b = (i % 7) as u8;
            }
            buf[break_at] = 0xFF;
            agree(&buf);
        }
    }

    #[test]
    fn matches_the_byte_loop_on_a_constant_window() {
        // The degenerate case: everything agrees, so the scan always runs to
        // the limit and the tail after the last whole word is what decides.
        agree(&[7u8; 40]);
    }

    #[test]
    fn matches_the_byte_loop_on_an_overlapping_run() {
        // `diff` smaller than the run is the overlapping copy LZMA emits most,
        // and the one where reading ahead of the cursor could see bytes the
        // byte loop has not reached.
        let mut buf = Vec::new();
        for i in 0..40u8 {
            buf.push(i % 3);
        }
        agree(&buf);
    }

    #[test]
    fn matches_the_byte_loop_on_pseudo_random_bytes() {
        // Short runs, which is what an incompressible input gives the finder.
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut buf = Vec::new();
        for _ in 0..48 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            buf.push((state >> 33) as u8);
        }
        agree(&buf);
    }

    #[test]
    fn empty_and_inverted_ranges_answer_start() {
        let buf = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        assert_eq!(match_run(&buf, 1, 5, 5), 5);
        assert_eq!(match_run(&buf, 1, 6, 5), 6);
    }
}
