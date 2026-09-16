//! The adaptive decoder: fed input, switchable mid-stream, bounded.
//!
//! One test per constraint the consumer's "adaptive chase" places on it.

mod common;

use std::collections::BTreeMap;

use lzma_fast::{DrainStatus, Error, Lzma2AdaptiveDecoder, Lzma2MtOptions};

use common::{copy_run, join_runs, multi_run, pseudo_random};

fn opts(threads: usize, memory_limit: u64) -> Lzma2MtOptions {
    Lzma2MtOptions {
        threads,
        memory_limit,
    }
}

/// Collects blocks by offset, checking that none of them overlap.
#[derive(Default)]
struct Sink {
    blocks: BTreeMap<u64, Vec<u8>>,
    order: Vec<u64>,
}

impl Sink {
    fn put(&mut self, offset: u64, bytes: &[u8]) {
        self.order.push(offset);
        let prev = self.blocks.insert(offset, bytes.to_vec());
        assert!(prev.is_none(), "two blocks at offset {offset}");
    }

    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (off, b) in &self.blocks {
            assert_eq!(*off, out.len() as u64, "gap or overlap at {off}");
            out.extend_from_slice(b);
        }
        out
    }
}

/// Feeds `packed` `chunk` bytes at a time, draining after each feed.
fn run_adaptive(
    dict_prop: u8,
    packed: &[u8],
    chunk: usize,
    threads: usize,
    limit: u64,
) -> Result<Sink, Error> {
    let mut dec = Lzma2AdaptiveDecoder::new(dict_prop, &opts(threads, limit))?;
    let mut sink = Sink::default();
    let mut pos = 0;
    loop {
        if pos < packed.len() {
            let end = (pos + chunk).min(packed.len());
            pos += dec.feed(&packed[pos..end])?;
            if pos == packed.len() {
                dec.end_of_input();
            }
        }
        assert!(
            dec.in_flight_bytes() <= limit.max(1 << 16),
            "in flight {} over limit {limit}",
            dec.in_flight_bytes()
        );
        let status = dec.drain(|off, b| sink.put(off, b))?;
        match status {
            DrainStatus::Finished => return Ok(sink),
            DrainStatus::NeedsMoreInput => {
                assert!(pos < packed.len(), "asked for input that does not exist");
            }
            DrainStatus::Progress => {}
        }
    }
}

// 1. Push/feed input, resumable, headers split across feeds.

#[test]
fn feeding_one_byte_at_a_time_decodes_the_same_stream() {
    let (prop, packed, plain) = multi_run(&["text.p1.xz", "rand.p1.xz", "mixed.p1.xz"], 2);
    for threads in [1usize, 4] {
        for chunk in [1usize, 3, 17, 4096] {
            let sink = run_adaptive(prop, &packed, chunk, threads, u64::MAX)
                .unwrap_or_else(|e| panic!("threads={threads} chunk={chunk}: {e}"));
            assert_eq!(sink.bytes(), plain, "threads={threads} chunk={chunk}");
        }
    }
}

// 2. The backlog of complete runs is visible to the caller.

#[test]
fn the_backlog_of_complete_runs_is_visible() {
    let (prop, packed, _) = multi_run(&["text.p1.xz", "mixed.p1.xz"], 3);
    let mut dec = Lzma2AdaptiveDecoder::new(prop, &opts(1, u64::MAX)).expect("props");
    // Feed everything but do not decode: the scanner still has to have walked
    // it before the backlog can be reported, which the first drain does.
    dec.feed(&packed).expect("feed");
    let mut seen = 0usize;
    let _ = dec.drain(|_, b| seen += b.len());
    // Six runs went in; whatever is left unclaimed plus what was claimed is
    // all of them.
    assert_eq!(dec.runs_claimed() + dec.pending_runs() as u64, 6);
    assert!(dec.backlog().all(|r| r.has_dict_reset));
}

// 3. Switching modes at a run boundary is lossless.

#[test]
fn switching_from_st_to_mt_at_any_run_boundary_is_lossless() {
    let names = ["text.p1.xz", "mixed.p1.xz", "rand.p1.xz", "zeros.p1.xz"];
    let (prop, packed, plain) = multi_run(&names, 2);
    let runs = names.len() * 2;

    for n in 0..=runs {
        let mut dec = Lzma2AdaptiveDecoder::new(prop, &opts(1, u64::MAX)).expect("props");
        let mut sink = Sink::default();
        let mut pos = 0usize;
        let mut switched = false;
        loop {
            // Decode the first `n` runs on the calling thread, the rest on
            // workers. No input is re-fed and nothing is re-decoded.
            if !switched && dec.runs_claimed() >= n as u64 {
                dec.set_threads(8);
                switched = true;
            }
            if pos < packed.len() {
                let end = (pos + 997).min(packed.len());
                pos += dec.feed(&packed[pos..end]).expect("feed");
                if pos == packed.len() {
                    dec.end_of_input();
                }
            }
            if dec.drain(|o, b| sink.put(o, b)).expect("drain") == DrainStatus::Finished {
                break;
            }
        }
        assert_eq!(sink.bytes(), plain, "n={n}");
    }
}

#[test]
fn switching_back_to_single_threaded_mid_stream_is_lossless() {
    let names = ["text.p1.xz", "mixed.p1.xz", "rand.p1.xz"];
    let (prop, packed, plain) = multi_run(&names, 3);
    let mut dec = Lzma2AdaptiveDecoder::new(prop, &opts(4, u64::MAX)).expect("props");
    let mut sink = Sink::default();
    let mut pos = 0usize;
    loop {
        // Flap between modes at every opportunity.
        dec.set_threads(if dec.runs_claimed().is_multiple_of(2) {
            1
        } else {
            4
        });
        if pos < packed.len() {
            let end = (pos + 1024).min(packed.len());
            pos += dec.feed(&packed[pos..end]).expect("feed");
            if pos == packed.len() {
                dec.end_of_input();
            }
        }
        if dec.drain(|o, b| sink.put(o, b)).expect("drain") == DrainStatus::Finished {
            break;
        }
    }
    assert_eq!(sink.bytes(), plain);
}

// 4. Ordered by default, unordered on request; both carry offsets.

#[test]
fn ordered_delivery_is_the_default_and_unordered_is_opt_in() {
    let (prop, packed, plain) = multi_run(&["text.p1.xz", "mixed.p1.xz"], 4);

    let ordered = run_adaptive(prop, &packed, 8192, 8, u64::MAX).expect("ordered");
    assert_eq!(ordered.bytes(), plain);
    let mut sorted = ordered.order.clone();
    sorted.sort_unstable();
    assert_eq!(ordered.order, sorted, "default delivery was out of order");

    let mut dec = Lzma2AdaptiveDecoder::new(prop, &opts(8, u64::MAX)).expect("props");
    dec.set_ordered(false);
    let mut sink = Sink::default();
    let mut pos = 0usize;
    loop {
        if pos < packed.len() {
            let end = (pos + 8192).min(packed.len());
            pos += dec.feed(&packed[pos..end]).expect("feed");
            if pos == packed.len() {
                dec.end_of_input();
            }
        }
        if dec.drain(|o, b| sink.put(o, b)).expect("drain") == DrainStatus::Finished {
            break;
        }
    }
    // Whatever order the blocks arrived in, their offsets place them.
    assert_eq!(sink.bytes(), plain);
}

// 5. Threads are spawned on first dispatch and parked, not torn down.

#[test]
fn threads_are_created_on_first_dispatch_and_survive_a_mode_change() {
    let (prop, packed, plain) = multi_run(&["text.p1.xz", "mixed.p1.xz"], 4);

    let mut dec = Lzma2AdaptiveDecoder::new(prop, &opts(4, u64::MAX)).expect("props");
    assert_eq!(dec.spawned_threads(), 0, "threads before any work");

    // Arriving in quarters, so that each drain sees a backlog of complete runs
    // rather than the tail of one still arriving. That backlog is the whole
    // reason to go multi-threaded, and the caller is the thing that sees it.
    let mut sink = Sink::default();
    let mut pos = 0usize;
    let mut peak = 0usize;
    let mut flip = false;
    let quarter = packed.len() / 4 + 1;
    loop {
        if pos < packed.len() {
            let end = (pos + quarter).min(packed.len());
            pos += dec.feed(&packed[pos..end]).expect("feed");
            if pos == packed.len() {
                dec.end_of_input();
            }
        }
        // Flap the mode across the whole decode. Dropping to one thread must
        // not cost the workers that already exist.
        flip = !flip;
        dec.set_threads(if flip { 4 } else { 1 });
        assert!(
            dec.spawned_threads() >= peak,
            "a worker was torn down by a mode change"
        );
        peak = peak.max(dec.spawned_threads());
        if dec.drain(|o, b| sink.put(o, b)).expect("drain") == DrainStatus::Finished {
            break;
        }
    }
    assert!(dec.spawned_threads() > 0, "nothing was ever dispatched");
    assert!(peak > 0, "the mode flap never saw a live worker");
    assert_eq!(sink.bytes(), plain);
}

#[test]
fn a_single_threaded_decoder_never_creates_a_thread() {
    let (prop, packed, plain) = multi_run(&["text.p1.xz", "mixed.p1.xz"], 2);
    let mut dec = Lzma2AdaptiveDecoder::new(prop, &opts(1, u64::MAX)).expect("props");
    dec.feed(&packed).expect("feed");
    dec.end_of_input();
    let mut sink = Sink::default();
    while dec.drain(|o, b| sink.put(o, b)).expect("drain") != DrainStatus::Finished {
        assert_eq!(dec.spawned_threads(), 0);
    }
    assert_eq!(dec.spawned_threads(), 0);
    assert_eq!(sink.bytes(), plain);
}

// 6. Memory is accounted, bounded, and the decode can be cancelled.

#[test]
fn a_memory_limit_bounds_what_is_in_flight() {
    // Runs of 1 MiB of incompressible bytes: eight of them is 8 MiB of output,
    // which a 2 MiB limit cannot hold even two of at once.
    let runs: Vec<_> = (0..8)
        .map(|i| copy_run(&pseudo_random(1 << 20, i + 1)))
        .collect();
    let (packed, plain) = join_runs(&runs);
    const LIMIT: u64 = 2 << 20;

    let mut dec = Lzma2AdaptiveDecoder::new(16, &opts(8, LIMIT)).expect("props");
    let mut sink = Sink::default();
    let mut pos = 0usize;
    let mut peak = 0u64;
    loop {
        if pos < packed.len() {
            let end = (pos + (1 << 16)).min(packed.len());
            let took = dec.feed(&packed[pos..end]).expect("feed");
            pos += took;
            if pos == packed.len() {
                dec.end_of_input();
            }
        }
        peak = peak.max(dec.in_flight_bytes());
        assert!(
            dec.in_flight_bytes() <= LIMIT,
            "in flight {} over the {LIMIT} byte limit",
            dec.in_flight_bytes()
        );
        if dec.drain(|o, b| sink.put(o, b)).expect("drain") == DrainStatus::Finished {
            break;
        }
    }
    assert_eq!(sink.bytes(), plain);
    assert!(peak <= LIMIT);
}

#[test]
fn a_run_too_large_for_the_limit_is_decoded_rather_than_stalled() {
    // One 4 MiB run under a 1 MiB limit: nothing can be dispatched, so the
    // single-threaded path has to stream it.
    let (packed, plain) = join_runs(&[copy_run(&pseudo_random(4 << 20, 7))]);
    let sink = run_adaptive(16, &packed, 1 << 15, 8, 1 << 20).expect("decode");
    assert_eq!(sink.bytes(), plain);
}

#[test]
fn cancel_stops_and_joins() {
    let (prop, packed, _) = multi_run(&["text.p1.xz", "mixed.p1.xz"], 4);
    let mut dec = Lzma2AdaptiveDecoder::new(prop, &opts(4, u64::MAX)).expect("props");
    dec.feed(&packed).expect("feed");
    let mut n = 0usize;
    let _ = dec.drain(|_, b| n += b.len());
    dec.cancel();
    assert_eq!(dec.spawned_threads(), 0, "workers outlived cancel");
    assert_eq!(dec.feed(&packed), Err(Error::Cancelled));
    assert_eq!(dec.drain(|_, _| {}), Err(Error::Cancelled));
}

// 7. The chase: a run whose tail has not arrived still decodes.

#[test]
fn a_run_decodes_before_its_tail_arrives() {
    // A single run, so there is no boundary to wait for: output must come out
    // while the run is still incomplete, or a chasing caller would see nothing
    // until the whole download finished.
    let (prop, packed, plain) = multi_run(&["text.p1.xz"], 1);
    let mut dec = Lzma2AdaptiveDecoder::new(prop, &opts(8, u64::MAX)).expect("props");
    let mut sink = Sink::default();

    let half = packed.len() / 2;
    dec.feed(&packed[..half]).expect("feed");
    let status = dec.drain(|o, b| sink.put(o, b)).expect("drain");
    assert_eq!(status, DrainStatus::Progress);
    let early = sink.bytes().len();
    assert!(early > 0, "nothing decoded from a run still arriving");
    assert_eq!(dec.pending_runs(), 0, "an incomplete run is not a backlog");
    assert_eq!(
        dec.spawned_threads(),
        0,
        "an incomplete run went to a worker"
    );

    dec.feed(&packed[half..]).expect("feed");
    dec.end_of_input();
    while dec.drain(|o, b| sink.put(o, b)).expect("drain") != DrainStatus::Finished {}
    assert_eq!(sink.bytes(), plain);
    assert!(early < plain.len(), "the whole run decoded before its tail");
}

#[test]
fn the_threaded_path_never_claims_a_run_the_chase_started() {
    // Feed most of a run, decode it single-threaded, then let the rest arrive
    // with threads available. The run is already part decoded, so it must stay
    // with the chase decoder rather than being handed to a worker, which would
    // decode it a second time and duplicate its output.
    let names = ["text.p1.xz", "mixed.p1.xz", "rand.p1.xz"];
    let (prop, packed, plain) = multi_run(&names, 2);
    let mut dec = Lzma2AdaptiveDecoder::new(prop, &opts(1, u64::MAX)).expect("props");
    let mut sink = Sink::default();

    let mut pos = 0usize;
    loop {
        if pos < packed.len() {
            // Small feeds keep the chase permanently part way through a run.
            let end = (pos + 512).min(packed.len());
            pos += dec.feed(&packed[pos..end]).expect("feed");
            if pos == packed.len() {
                dec.end_of_input();
            }
        }
        // Threads become available half way through, mid-run.
        if pos > packed.len() / 2 {
            dec.set_threads(8);
        }
        if dec.drain(|o, b| sink.put(o, b)).expect("drain") == DrainStatus::Finished {
            break;
        }
    }
    assert_eq!(sink.bytes(), plain);
}
