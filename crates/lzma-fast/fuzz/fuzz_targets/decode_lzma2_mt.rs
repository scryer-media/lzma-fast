//! Arbitrary bytes as a raw LZMA2 stream, decoded three ways at once: the
//! single-threaded decoder, the parallel decoder and the adaptive one.
//!
//! The property is agreement, not merely absence of panics. A stream either
//! ends at an end marker, in which case all three must produce the same bytes,
//! or it does not, in which case all three must refuse it. That catches the
//! failure mode the multi-threaded path is actually prone to - a worker that
//! silently stops early, or a parse error swallowed into a short but
//! successful decode - which a panic-only target would not.
#![no_main]

use std::io::Write;

use libfuzzer_sys::fuzz_target;
use lzma_fast::{
    DrainStatus, FinishMode, Lzma2AdaptiveDecoder, Lzma2Decoder, Lzma2MtOptions,
    Lzma2ParallelDecoder, Status,
};

/// Bound on the output any of the three is allowed to produce before the case
/// is abandoned: a fuzzer input is a few kilobytes, and a decompression bomb
/// is not what this target is looking for.
const MAX_OUT: usize = 1 << 24;
/// Bound on what the parallel decoder may hold in flight.
const MEM_LIMIT: u64 = 1 << 26;

fuzz_target!(|data: &[u8]| {
    if data.len() < 3 {
        return;
    }
    let threads = usize::from(data[0] % 4) + 1;
    // Cap the dictionary so the fuzzer cannot drive a multi-gigabyte alloc.
    let dict_prop = data[1] % 27;
    let feed = match data[2] % 4 {
        0 => 1usize,
        1 => 13,
        2 => 997,
        _ => usize::MAX,
    };
    // The chase knob: a caller decoding a file already on disk turns it off,
    // and must still get the same bytes, and still finish.
    let chase = data[0] & 0x80 == 0;
    // Every drain is bounded on some inputs, which cuts blocks in half and
    // makes the decoder carry the remainder.
    let drain_limit = match data[1] % 5 {
        0 => 1usize,
        1 => 7,
        2 => 65_536,
        _ => usize::MAX,
    };
    let stream = &data[3..];

    let Some(expected) = single_threaded(dict_prop, stream) else {
        return;
    };

    // Parallel, pushed through the whole-stream API.
    let opts = Lzma2MtOptions {
        threads,
        memory_limit: MEM_LIMIT,
    };
    let Ok(dec) = Lzma2ParallelDecoder::new(dict_prop, &opts) else {
        return;
    };
    let mut got = Vec::new();
    let res = dec.decode(stream, Capped(&mut got));
    check("parallel", &expected, res.is_ok(), &got);

    // Adaptive, fed in slices, which is the shape a caller chasing a download
    // uses.
    let Ok(mut ad) = Lzma2AdaptiveDecoder::new(dict_prop, &opts) else {
        return;
    };
    ad.set_chase(chase);
    let mut out = Vec::new();
    let ok = adaptive(&mut ad, stream, feed, drain_limit, &mut out);
    check("adaptive", &expected, ok, &out);
});

/// The reference: `Some(bytes)` only when the stream ends at an end marker,
/// `None` when it is corrupt, truncated, or too large to be worth comparing.
fn single_threaded(dict_prop: u8, mut input: &[u8]) -> Option<Option<Vec<u8>>> {
    let mut dec = Lzma2Decoder::new(dict_prop).ok()?;
    let mut out = vec![0u8; 4096];
    let mut acc: Vec<u8> = Vec::new();
    loop {
        let Ok(p) = dec.decode(input, &mut out, FinishMode::Any) else {
            return Some(None);
        };
        input = &input[p.read..];
        acc.extend_from_slice(&out[..p.written]);
        if acc.len() > MAX_OUT {
            return None;
        }
        if p.status == Status::FinishedWithMark {
            return Some(Some(acc));
        }
        if p.read == 0 && p.written == 0 {
            // Out of input without an end marker: truncated.
            return Some(None);
        }
    }
}

fn adaptive(
    ad: &mut Lzma2AdaptiveDecoder,
    mut stream: &[u8],
    feed: usize,
    drain_limit: usize,
    out: &mut Vec<u8>,
) -> bool {
    let mut failed = false;
    loop {
        if !stream.is_empty() {
            let n = feed.min(stream.len());
            match ad.feed(&stream[..n]) {
                Ok(used) => stream = &stream[used..],
                Err(_) => return false,
            }
            if stream.is_empty() {
                ad.end_of_input();
            }
        }
        let status = ad.drain_upto(drain_limit, |off, bytes| {
            let off = off as usize;
            if off + bytes.len() > MAX_OUT {
                failed = true;
                return;
            }
            if out.len() < off + bytes.len() {
                out.resize(off + bytes.len(), 0);
            }
            out[off..off + bytes.len()].copy_from_slice(bytes);
        });
        if failed {
            return false;
        }
        match status {
            Ok(DrainStatus::Finished) => return true,
            Ok(DrainStatus::NeedsMoreInput) if stream.is_empty() => return false,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
}

fn check(who: &str, expected: &Option<Vec<u8>>, ok: bool, got: &[u8]) {
    match expected {
        Some(want) => {
            assert!(
                ok,
                "{who}: rejected a stream the single-threaded decoder accepted"
            );
            assert_eq!(got, want.as_slice(), "{who}: decoded different bytes");
        }
        None => assert!(
            !ok,
            "{who}: accepted a stream the single-threaded decoder rejected"
        ),
    }
}

/// A sink that stops growing past [`MAX_OUT`]; the single-threaded reference
/// has already established the length, so anything longer is a bug rather than
/// a case to explore.
struct Capped<'a>(&'a mut Vec<u8>);

impl Write for Capped<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        assert!(
            self.0.len() + buf.len() <= MAX_OUT,
            "parallel decoder overran"
        );
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
