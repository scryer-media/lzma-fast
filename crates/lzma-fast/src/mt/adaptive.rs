//! Decoding a stream that is still arriving.
//!
//! [`super`] decodes a stream the way 7-Zip does: hand it a reader, get bytes
//! out, and every thread the plan asked for runs for the whole call. That is
//! the fastest way to decode an archive that is already on disk, and it is
//! what the throughput numbers in `docs/perf-log.md` are measured against.
//!
//! It is the wrong shape for a caller decoding an archive while it downloads.
//! Such a caller wants to stay single-threaded while it is chasing the tail of
//! the byte flow — decode what has arrived, cheaply, with one decoder and no
//! blocks in flight — and to spread across threads only once a backlog of
//! fully-arrived runs has built up, and then to go back. It cannot block on a
//! reader, because there is nothing to read yet; it decides what to do next
//! from what it can see, so it needs to see the backlog.
//!
//! [`Lzma2AdaptiveDecoder`] is that shape: bytes go in with
//! [`feed`](Lzma2AdaptiveDecoder::feed), which never blocks, output comes out
//! of [`drain`](Lzma2AdaptiveDecoder::drain) as (offset, bytes) blocks, and
//! [`set_threads`](Lzma2AdaptiveDecoder::set_threads) changes the mode at the
//! next run boundary without re-decoding or re-buffering anything. Both modes
//! decode the identical runs found by the identical [`Lzma2RunScanner`], so
//! switching between them cannot change the output.
//!
//! C: nothing. See [`super::pool`] for why the `MtDec` ring could not be bent
//! into this shape, and `docs/porting.md` for the record of the deviation.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use crate::error::{Error, FinishMode, Status};
use crate::lzma2::Lzma2Decoder;

use crate::lzma2::scan::{Lzma2Run, Lzma2RunScanner};
use crate::mt::Lzma2MtOptions;
#[cfg(feature = "crc")]
use crate::mt::checksum::{BlockChecks, ChecksumPlan, Segmenter};
use crate::mt::pool::{Done, Job, Pool, locate};

/// How much of the single-threaded decoder's dictionary is filled before it is
/// handed to the caller. C: `props.outStep_ST`.
const OUT_STEP_ST: usize = 1 << 20;

/// Why [`Lzma2AdaptiveDecoder::drain`] stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainStatus {
    /// Everything that could be decoded from the bytes fed so far has been.
    /// Feed more.
    NeedsMoreInput,
    /// Output was produced and there may be more immediately available; the
    /// caller got control back so that it can reconsider the thread count.
    Progress,
    /// The stream's end marker was reached and every block has been delivered.
    Finished,
}

/// A block of decoded output, in the order the caller asked for.
type Ready = (u64, Vec<u8>, usize);

/// What came of trying to hand the run at the cursor to a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dispatch {
    /// A worker has it.
    Sent,
    /// There is a run to dispatch but no room to dispatch it: every worker is
    /// busy, or the memory limit is reached with blocks still in flight. Wait
    /// for one rather than decoding it on the caller's thread.
    Busy,
    /// Nothing to dispatch: no complete run at the cursor, or one that the
    /// chase decoder should take instead.
    None,
}

/// An LZMA2 decoder that is fed input and switches between single- and
/// multi-threaded decoding while it runs.
pub struct Lzma2AdaptiveDecoder {
    dict_prop: u8,
    memory_limit: u64,
    threads: usize,
    ordered: bool,

    // Input. `buf[0]` is at stream offset `base`; nothing before `cursor_in`
    // is retained.
    buf: Vec<u8>,
    base: u64,
    input_done: bool,

    // Run discovery.
    scanner: Lzma2RunScanner,
    pending: VecDeque<Lzma2Run>,
    /// Offset of the stream's end marker, once it has been found.
    stream_end: Option<u64>,

    // The one cursor both modes claim work at. Everything before it has been
    // claimed by exactly one of them, in stream order.
    cursor_in: u64,
    cursor_out: u64,
    next_index: u64,
    runs_claimed: u64,

    // The single-threaded chase decoder, built on first use.
    st: Option<Lzma2Decoder>,
    st_wr: usize,
    /// True while the single-threaded decoder is part way through a run. The
    /// threaded path must not claim a run it has started.
    st_in_run: bool,

    // The worker pool, spawned on first dispatch.
    pool: Option<Pool>,
    outstanding: usize,
    outstanding_bytes: u64,

    // Decoded blocks waiting for their turn.
    ready: BTreeMap<u64, Ready>,
    ready_bytes: u64,
    emitted_out: u64,
    spare_out: Vec<Vec<u8>>,
    spare_in: Vec<Vec<u8>>,

    // What each run's decoder checksummed, worker or chase alike. See
    // [`crate::checksum`]: this is computed where the bytes were produced,
    // never on the caller's thread as it drains.
    #[cfg(feature = "crc")]
    plan: ChecksumPlan,
    #[cfg(feature = "crc")]
    checks: Vec<BlockChecks>,
    /// The chase decoder's open segment run, carried across calls so a run it
    /// decodes in many steps yields the segments the split points ask for and
    /// not one per step.
    #[cfg(feature = "crc")]
    st_seg: Option<Segmenter>,

    /// A worker's error, held until everything before it has been delivered.
    failed: Option<(u64, Error)>,
    complete: bool,
    cancelled: bool,
}

impl core::fmt::Debug for Lzma2AdaptiveDecoder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Lzma2AdaptiveDecoder")
            .field("dict_prop", &self.dict_prop)
            .field("threads", &self.threads)
            .field("ordered", &self.ordered)
            .field("in_flight_bytes", &self.in_flight_bytes())
            .field("pending_runs", &self.pending.len())
            .field("outstanding", &self.outstanding)
            .finish_non_exhaustive()
    }
}

impl Lzma2AdaptiveDecoder {
    /// A decoder for a raw LZMA2 stream with dictionary property byte
    /// `dict_prop`.
    ///
    /// No threads are created here, and none are created until the first run
    /// is actually dispatched to one.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedProps`] if `dict_prop > 40`.
    pub fn new(dict_prop: u8, options: &Lzma2MtOptions) -> Result<Self, Error> {
        if dict_prop > 40 {
            return Err(Error::UnsupportedProps);
        }
        Ok(Lzma2AdaptiveDecoder {
            dict_prop,
            memory_limit: options.memory_limit,
            threads: options.threads.max(1),
            ordered: true,
            buf: Vec::new(),
            base: 0,
            input_done: false,
            scanner: Lzma2RunScanner::new(),
            pending: VecDeque::new(),
            stream_end: None,
            cursor_in: 0,
            cursor_out: 0,
            next_index: 0,
            runs_claimed: 0,
            st: None,
            st_wr: 0,
            st_in_run: false,
            pool: None,
            outstanding: 0,
            outstanding_bytes: 0,
            ready: BTreeMap::new(),
            ready_bytes: 0,
            emitted_out: 0,
            spare_out: Vec::new(),
            spare_in: Vec::new(),
            #[cfg(feature = "crc")]
            plan: ChecksumPlan::none(),
            #[cfg(feature = "crc")]
            checks: Vec::new(),
            #[cfg(feature = "crc")]
            st_seg: None,
            failed: None,
            complete: false,
            cancelled: false,
        })
    }

    // -- mode ---------------------------------------------------------------

    /// Sets the ceiling on threads used from the next run boundary onwards.
    ///
    /// One means the next run is decoded inline on the calling thread: no
    /// worker is woken, nothing is dispatched, and the output is handed to the
    /// sink straight out of the decoder's dictionary. Already-dispatched runs
    /// are not recalled; already-spawned workers stay parked on their channel
    /// and cost nothing until the count goes back up.
    pub fn set_threads(&mut self, threads: usize) {
        self.threads = threads.max(1);
    }

    /// The current thread ceiling.
    #[must_use]
    pub fn threads(&self) -> usize {
        self.threads
    }

    /// How many worker threads exist. Zero until the first dispatch.
    #[must_use]
    pub fn spawned_threads(&self) -> usize {
        self.pool.as_ref().map_or(0, Pool::spawned)
    }

    /// How many worker threads are running right now.
    ///
    /// Equal to [`Lzma2AdaptiveDecoder::spawned_threads`] until the decoder is
    /// cancelled or dropped, at which point it must fall to zero.
    #[must_use]
    pub fn live_threads(&self) -> usize {
        self.pool.as_ref().map_or(0, Pool::live)
    }

    /// Sets what each decoder - worker or chase - is to checksum over the
    /// bytes it produces, and where the consumer's boundaries fall.
    ///
    /// Takes effect for runs claimed after this call, so set it before the
    /// first [`Lzma2AdaptiveDecoder::drain`]. The checksums are computed
    /// wherever the bytes were produced and never on the thread draining the
    /// output; see [`crate::checksum`] for why that distinction is the
    /// whole point.
    #[cfg(feature = "crc")]
    pub fn set_checksum(&mut self, plan: &ChecksumPlan) {
        self.plan = plan.clone();
    }

    /// Everything checksummed since the last call.
    ///
    /// Entries appear as their runs finish, which with unordered delivery is
    /// not stream order; each carries its own absolute offset, so a
    /// [`crate::crc::CrcFolder`] can take them as they come. After the decode
    /// is complete this has returned one entry per run, tiling the output.
    #[cfg(feature = "crc")]
    pub fn take_checks(&mut self) -> Vec<BlockChecks> {
        if self.complete {
            self.close_st_seg();
        }
        core::mem::take(&mut self.checks)
    }

    /// Closes the chase decoder's open segment run, if it has one.
    #[cfg(feature = "crc")]
    fn close_st_seg(&mut self) {
        if let Some(seg) = self.st_seg.take() {
            let checks = seg.finish();
            if checks.len != 0 {
                self.checks.push(checks);
            }
        }
    }

    /// Delivers blocks in stream order (the default), or as soon as they are
    /// decoded.
    ///
    /// Unordered delivery suits a sink that writes by offset: a run whose
    /// bytes arrived early can be written out while an earlier run is still
    /// being decoded. Every block carries its output offset either way.
    pub fn set_ordered(&mut self, ordered: bool) {
        self.ordered = ordered;
    }

    // -- what the caller decides on -----------------------------------------

    /// Complete runs that have arrived and not yet been claimed: the backlog.
    #[must_use]
    pub fn pending_runs(&self) -> usize {
        self.pending.len()
    }

    /// The complete runs that have arrived and not yet been claimed.
    #[must_use]
    pub fn backlog(&self) -> impl ExactSizeIterator<Item = &Lzma2Run> {
        self.pending.iter()
    }

    /// How many runs have been handed to a decoder, by either path.
    #[must_use]
    pub fn runs_claimed(&self) -> u64 {
        self.runs_claimed
    }

    /// Bytes the decoder is currently holding: buffered input, blocks being
    /// decoded, and blocks decoded but not yet delivered.
    ///
    /// Dispatch is refused rather than allowed to push this over
    /// [`Lzma2AdaptiveDecoder::memory_limit`]; a run too large to fit at all
    /// is decoded by the single-threaded path, which streams it, instead of
    /// stalling.
    #[must_use]
    pub fn in_flight_bytes(&self) -> u64 {
        self.buf.len() as u64 + self.outstanding_bytes + self.ready_bytes
    }

    /// The limit [`Lzma2AdaptiveDecoder::in_flight_bytes`] is kept under.
    #[must_use]
    pub fn memory_limit(&self) -> u64 {
        self.memory_limit
    }

    // -- input --------------------------------------------------------------

    /// Adds `data` to the stream and returns how much of it was taken.
    ///
    /// Never blocks and never decodes. Takes less than all of `data` only when
    /// the memory limit is reached, in which case the caller should drain and
    /// feed the rest.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Cancelled`] after [`Lzma2AdaptiveDecoder::cancel`].
    pub fn feed(&mut self, data: &[u8]) -> Result<usize, Error> {
        if self.cancelled {
            return Err(Error::Cancelled);
        }
        if data.is_empty() || self.complete {
            return Ok(0);
        }
        let held = self.in_flight_bytes();
        let room = self.memory_limit.saturating_sub(held);
        // Always take at least one byte's worth of progress possible: a limit
        // smaller than the buffer would otherwise deadlock. The single-threaded
        // path streams, so a large buffer is never required to make progress.
        let take = usize::try_from(room).unwrap_or(usize::MAX).min(data.len());
        let take = if take == 0 && self.buf.is_empty() {
            data.len().min(1 << 16)
        } else {
            take
        };
        self.buf.extend_from_slice(&data[..take]);
        Ok(take)
    }

    /// Declares that no more input is coming. A stream that then does not end
    /// with its end marker is reported as corrupt rather than waited on.
    pub fn end_of_input(&mut self) {
        self.input_done = true;
    }

    /// Stops the decode: workers finish whatever run they hold, are told to do
    /// no more, and are joined before this returns.
    pub fn cancel(&mut self) {
        self.cancelled = true;
        if let Some(p) = self.pool.as_mut() {
            p.shutdown();
        }
        self.pool = None;
        self.outstanding = 0;
        self.outstanding_bytes = 0;
    }

    // -- output -------------------------------------------------------------

    /// Decodes as much as the bytes fed so far allow and hands each block to
    /// `sink` as `(output offset, bytes)`.
    ///
    /// Returns when there is nothing left to do without more input, when the
    /// stream is finished, or when a block has been delivered and the caller
    /// might want to reconsider the thread count. It blocks only while a
    /// worker is decoding and there is nothing else to get on with.
    ///
    /// # Errors
    ///
    /// Returns the located [`Error::CorruptRun`] for a run that does not
    /// decode, but only after every block before it has been delivered, so a
    /// caller writing by offset keeps what it has already written.
    pub fn drain<F>(&mut self, mut sink: F) -> Result<DrainStatus, Error>
    where
        F: FnMut(u64, &[u8]),
    {
        if self.cancelled {
            return Err(Error::Cancelled);
        }
        let mut progress = false;
        loop {
            self.scan()?;
            let mut did = self.collect(false);
            did |= self.emit(&mut sink);
            self.check_failed()?;

            // The chase decoder must not steal a run that a worker could take,
            // or a decoder with four idle workers would do all the work on the
            // caller's thread. It takes over only when there is no complete
            // run at the cursor to dispatch, which is exactly the case it
            // exists for: the tail of the stream, still arriving.
            match self.dispatch()? {
                Dispatch::Sent => did = true,
                Dispatch::Busy => {}
                Dispatch::None => {
                    if self.st_step(&mut sink)? {
                        did = true;
                    }
                }
            }

            progress |= did;

            if self.complete && self.outstanding == 0 && self.ready.is_empty() {
                return Ok(DrainStatus::Finished);
            }
            if did {
                continue;
            }
            if self.outstanding != 0 {
                // A worker is busy and there is nothing else to do, so wait
                // for it rather than spin.
                if self.collect(true) {
                    continue;
                }
                return Err(Error::InternalFailure);
            }
            if self.input_done && !self.complete {
                return Err(Error::CorruptData);
            }
            return Ok(if progress {
                DrainStatus::Progress
            } else {
                DrainStatus::NeedsMoreInput
            });
        }
    }

    // -- the machinery ------------------------------------------------------

    /// Walks whatever headers have arrived since the last call.
    fn scan(&mut self) -> Result<(), Error> {
        if self.scanner.finished() {
            return Ok(());
        }
        let from = usize::try_from(self.scanner.in_position() - self.base)
            .map_err(|_| Error::InternalFailure)?;
        if from >= self.buf.len() {
            return Ok(());
        }
        self.scanner.feed(&self.buf[from..])?;
        while let Some(r) = self.scanner.next_run() {
            self.pending.push_back(r);
        }
        if self.scanner.finished() {
            self.stream_end = Some(self.scanner.in_position());
        }
        Ok(())
    }

    /// Drops input nothing will ask for again.
    fn compact(&mut self) {
        let keep_from = self.cursor_in;
        if keep_from <= self.base {
            return;
        }
        let drop = (keep_from - self.base) as usize;
        if drop < self.buf.len() / 2 && drop < (1 << 20) {
            return;
        }
        self.buf.drain(..drop);
        self.base = keep_from;
    }

    /// Takes finished blocks off the pool, optionally waiting for one.
    fn collect(&mut self, block: bool) -> bool {
        let Some(pool) = self.pool.as_ref() else {
            return false;
        };
        let mut batch = Vec::new();
        if block && let Some(d) = pool.collect() {
            batch.push(d);
        }
        while let Some(d) = pool.try_collect() {
            batch.push(d);
        }
        let any = !batch.is_empty();
        for d in batch {
            self.accept(d);
        }
        any
    }

    fn accept(&mut self, d: Done) {
        self.outstanding -= 1;
        self.outstanding_bytes -= d.unpacked_len as u64;
        self.spare_in.push(d.packed);
        match d.res {
            Ok(()) => {
                #[cfg(feature = "crc")]
                if let Some(c) = d.checks {
                    self.checks.push(c);
                }
                self.ready_bytes += d.unpacked_len as u64;
                self.ready
                    .insert(d.out_offset, (d.out_offset, d.out, d.unpacked_len));
            }
            Err(e) => {
                self.recycle(d.out);
                let located = locate(d.index, d.out_offset, e);
                match &self.failed {
                    Some((off, _)) if *off <= d.out_offset => {}
                    _ => self.failed = Some((d.out_offset, located)),
                }
            }
        }
    }

    /// Hands out every block whose turn has come.
    fn emit<F: FnMut(u64, &[u8])>(&mut self, sink: &mut F) -> bool {
        let mut any = false;
        loop {
            let key = if self.ordered {
                match self.ready.first_key_value() {
                    Some((k, _)) if *k == self.emitted_out => *k,
                    _ => break,
                }
            } else {
                match self.ready.first_key_value() {
                    Some((k, _)) => *k,
                    None => break,
                }
            };
            let (off, buf, len) = self.ready.remove(&key).expect("just looked it up");
            self.ready_bytes -= len as u64;
            sink(off, &buf[..len]);
            if off == self.emitted_out {
                self.emitted_out = off + len as u64;
            }
            self.recycle(buf);
            any = true;
        }
        any
    }

    fn check_failed(&mut self) -> Result<(), Error> {
        if let Some((off, e)) = self.failed {
            // In order, everything before the bad run has now been delivered;
            // out of order there is nothing to wait for.
            if !self.ordered || self.emitted_out >= off {
                self.cancel();
                self.cancelled = false;
                return Err(e);
            }
        }
        Ok(())
    }

    fn recycle(&mut self, buf: Vec<u8>) {
        if self.spare_out.len() < self.threads + 2 {
            self.spare_out.push(buf);
        }
    }

    /// Sends the run at the cursor to a worker, if that is the right thing to
    /// do with it.
    fn dispatch(&mut self) -> Result<Dispatch, Error> {
        if self.threads <= 1 || self.complete || self.failed.is_some() || self.st_in_run {
            return Ok(Dispatch::None);
        }
        let Some(run) = self.pending.front().copied() else {
            return Ok(Dispatch::None);
        };
        if run.in_offset != self.cursor_in {
            return Ok(Dispatch::None);
        }
        if self.outstanding >= self.threads {
            return Ok(Dispatch::Busy);
        }
        let unpacked = usize::try_from(run.unpacked_len).map_err(|_| Error::Alloc)?;
        let packed = usize::try_from(run.packed_len).map_err(|_| Error::Alloc)?;

        // A run that cannot fit inside the limit at all is left to the
        // single-threaded path, which streams it out of its dictionary a
        // megabyte at a time. Waiting for room that will never appear would be
        // a deadlock, and buffering it anyway would be a lie about the limit.
        let need = run.unpacked_len + run.packed_len;
        if need > self.memory_limit {
            return Ok(Dispatch::None);
        }
        if self.in_flight_bytes() + need > self.memory_limit {
            // Room appears when an outstanding block lands. If none is
            // outstanding there is nothing to wait for, so the chase decoder
            // takes it and streams it instead.
            return Ok(if self.outstanding == 0 {
                Dispatch::None
            } else {
                Dispatch::Busy
            });
        }

        let from = (run.in_offset - self.base) as usize;
        if self.buf.len() < from + packed {
            return Ok(Dispatch::None);
        }

        let pool = match self.pool.as_mut() {
            Some(p) => p,
            None => {
                self.pool = Some(Pool::new(self.dict_prop));
                self.pool.as_mut().expect("just set")
            }
        };
        if pool.spawned() < self.threads && pool.spawned() <= self.outstanding {
            pool.grow();
        }
        if pool.spawned() == 0 {
            // No thread could be created; fall back to decoding inline.
            return Ok(Dispatch::None);
        }

        let mut packed_buf = self.spare_in.pop().unwrap_or_default();
        packed_buf.clear();
        packed_buf.extend_from_slice(&self.buf[from..from + packed]);
        let out = self.spare_out.pop().unwrap_or_default();

        pool.dispatch(Job {
            #[cfg(feature = "crc")]
            plan: self.plan.clone(),
            index: self.next_index,
            out_offset: run.out_offset,
            unpacked_len: unpacked,
            packed: packed_buf,
            out,
        })?;

        self.outstanding += 1;
        self.outstanding_bytes += run.unpacked_len;
        self.next_index += 1;
        self.runs_claimed += 1;
        self.pending.pop_front();
        self.cursor_in += run.packed_len;
        self.cursor_out += run.unpacked_len;
        self.note_end();
        self.compact();
        Ok(Dispatch::Sent)
    }

    /// Decodes on the calling thread: one step of at most
    /// [`OUT_STEP_ST`] bytes, over whatever input has arrived.
    ///
    /// This is the chase path. It decodes a run whose tail has not arrived
    /// yet, chunk by chunk, and keeps its state between calls, so there is no
    /// need to wait for a run to be complete before starting on it.
    fn st_step<F: FnMut(u64, &[u8])>(&mut self, sink: &mut F) -> Result<bool, Error> {
        if self.complete || self.failed.is_some() {
            return Ok(false);
        }
        let avail = self.buf.len() - (self.cursor_in - self.base) as usize;
        if avail == 0 {
            return Ok(false);
        }
        if self.st.is_none() {
            self.st = Some(Lzma2Decoder::new(self.dict_prop)?);
            self.st_wr = 0;
        }
        let dec = self.st.as_mut().expect("just built");

        let dic_pos = dec.dic_pos();
        let mut limit = dec.dic_buf_size();
        if limit - self.st_wr > OUT_STEP_ST {
            limit = self.st_wr + OUT_STEP_ST;
        }
        let from = (self.cursor_in - self.base) as usize;
        let (used, status) = dec.decode_block(limit, &self.buf[from..], FinishMode::Any)?;
        let produced = dec.dic_pos() - dic_pos;

        if produced != 0 {
            let start = self.st_wr;
            let end = dec.dic_pos();
            let offset = self.cursor_out;
            #[cfg(feature = "crc")]
            if !self.plan.is_none() {
                // A run the chase decoder takes over several steps must still
                // be cut only where the split points say, so the segmenter is
                // carried across steps and restarted only when the chase path
                // resumes somewhere else.
                if self.st_seg.as_ref().is_some_and(|s| s.pos() != offset) {
                    // Inlined `close_st_seg`: these are disjoint fields, and
                    // the decoder is borrowed across this block.
                    if let Some(seg) = self.st_seg.take() {
                        let checks = seg.finish();
                        if checks.len != 0 {
                            self.checks.push(checks);
                        }
                    }
                }
                let plan = self.plan.clone();
                self.st_seg
                    .get_or_insert_with(|| Segmenter::new(&plan, offset))
                    .update(dec.dic_slice(start, end));
            }
            if self.ordered && self.emitted_out != offset {
                // Blocks dispatched earlier have not landed yet, so this one
                // has to wait its turn in memory.
                let mut b = self.spare_out.pop().unwrap_or_default();
                b.clear();
                b.extend_from_slice(dec.dic_slice(start, end));
                let len = b.len();
                self.ready_bytes += len as u64;
                self.ready.insert(offset, (offset, b, len));
            } else {
                sink(offset, dec.dic_slice(start, end));
                self.emitted_out = offset + produced as u64;
            }
            dec.wrap_dic_pos();
            self.st_wr = dec.dic_pos();
        }

        self.cursor_in += used as u64;
        self.cursor_out += produced as u64;
        self.retire_runs();
        self.note_end();

        if status == Status::FinishedWithMark {
            self.complete = true;
            self.stream_end = Some(self.cursor_in);
        }
        if used == 0 && produced == 0 {
            if self.input_done && !self.complete {
                return Err(Error::CorruptData);
            }
            return Ok(false);
        }
        self.compact();
        Ok(true)
    }

    /// Drops runs the single-threaded path has decoded past, and works out
    /// whether the cursor is at a boundary.
    fn retire_runs(&mut self) {
        while let Some(front) = self.pending.front().copied() {
            if front.in_offset + front.packed_len <= self.cursor_in {
                self.pending.pop_front();
                self.runs_claimed += 1;
                self.next_index += 1;
            } else {
                break;
            }
        }
        self.st_in_run = match self.pending.front() {
            Some(front) => self.cursor_in > front.in_offset,
            // Past every complete run: either inside the run still arriving,
            // or exactly at the end marker.
            None => self.stream_end.is_none_or(|e| self.cursor_in + 1 < e),
        };
    }

    /// Notices that the threaded path has claimed the last run there is.
    fn note_end(&mut self) {
        if let Some(end) = self.stream_end
            && self.pending.is_empty()
            && !self.st_in_run
            && self.cursor_in + 1 >= end
        {
            self.complete = true;
            self.cursor_in = end;
        }
    }
}
