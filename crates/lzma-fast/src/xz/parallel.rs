//! Block-parallel decoding of a seekable `.xz` file.
//!
//! C: `C/XzDecMt.c`. The idea is the same - one thread reads blocks and hands
//! them out, workers decode whole blocks, the output is reassembled in order -
//! but the scheduling is simpler here because the index is read first: every
//! block's offset, compressed size and uncompressed size are known before a
//! byte is decoded, so there is no need for 7-Zip's "parse ahead and hope"
//! path or for its block-size guessing. The trade is that this reader needs a
//! seekable source; a pipe gets [`super::XzReader`], which is sequential.
//!
//! Memory is the other reason the index comes first. A worker costs its
//! block's output buffer (which is also its dictionary) plus the block as it
//! sits in the file, and the index says how big the largest of those is, so
//! the thread count can be *degraded to fit* the caller's limit instead of the
//! decode failing halfway through.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use std::io::{self, Read, Seek, SeekFrom};

use super::XzOptions;
use super::check::BlockCheck;
use super::error::{XzError, XzErrorKind, XzResult};
use super::index::{XzStreamIndex, read_stream_index_ending_at};
use super::pool::{XzDone, XzJob, XzPool};
use super::stream::{CheckType, STREAM_HEADER_SIZE};

/// What a worker costs besides its buffers: the LZMA2 probability table, which
/// is `1846 + (0x300 << (lc + lp))` sixteen-bit entries at the maximum
/// `lc + lp` LZMA2 allows, rounded up.
const PROBS_BYTES: u64 = 32 << 10;

/// One block of the file, as the index describes it.
#[derive(Debug, Clone, Copy)]
struct Plan {
    stream: u64,
    block_in_stream: u64,
    file_offset: u64,
    unpacked_offset: u64,
    unpacked_len: usize,
    unpadded_size: u64,
    padded_size: u64,
    check: CheckType,
}

/// A `Read` adapter that decodes an `.xz` file's blocks in parallel.
///
/// Construction reads the index of every stream in the file, which is where
/// all the structural validation happens: a file this cannot map is refused
/// here, before any thread is started, and the caller falls back to
/// [`super::XzReader`]. Decoding then reads blocks in file order, hands them
/// to workers, and reassembles the output in order.
///
/// The source has to be `Read + Seek`, but not `Send`: it is only ever read on
/// the caller's own thread.
///
/// # Example
///
/// ```no_run
/// use std::fs::File;
/// use std::io::Read;
/// use lzma_fast::xz::{XzOptions, XzParallelReader, XzReader};
///
/// # fn main() -> std::io::Result<()> {
/// let file = File::open("big.tar.xz")?;
/// let opts = XzOptions::default().with_threads(8).with_memory_limit(1 << 30);
/// let mut out = Vec::new();
/// match XzParallelReader::with_options(file, opts) {
///     Ok(mut r) => r.read_to_end(&mut out)?,
///     // Not a shape that can be decoded in parallel - a pipe-written file
///     // with no usable index, say. The sequential reader takes anything.
///     Err(_) => XzReader::new(File::open("big.tar.xz")?).read_to_end(&mut out)?,
/// };
/// # Ok(())
/// # }
/// ```
pub struct XzParallelReader<R: Read + Seek> {
    src: R,
    opts: XzOptions,
    blocks: Vec<Plan>,
    /// Where `src` is positioned, so a contiguous run of blocks is read
    /// without a seek between them.
    pos: u64,
    next_dispatch: usize,
    next_emit: u64,
    pool: XzPool,
    threads: usize,
    in_flight: usize,
    ready: BTreeMap<u64, XzDone>,
    /// The block being handed to the caller.
    current: Option<(Vec<u8>, usize, usize)>,
    spare_out: Vec<Vec<u8>>,
    spare_in: Vec<Vec<u8>>,
    checks: Vec<BlockCheck>,
    total_out: u64,
    per_worker: u64,
    failed: bool,
}

impl<R: Read + Seek> XzParallelReader<R> {
    /// Maps the file and prepares to decode it with the default options.
    ///
    /// # Errors
    ///
    /// Any structural error in any stream's footer, index or header, and
    /// [`XzErrorKind::MemoryLimit`] if even one worker would not fit.
    pub fn new(src: R) -> XzResult<Self> {
        Self::with_options(src, XzOptions::default())
    }

    /// As [`XzParallelReader::new`], with options.
    ///
    /// # Errors
    ///
    /// As [`XzParallelReader::new`], plus [`XzErrorKind::TooMuchOutput`] if
    /// the index says the file decodes to more than the caller allows - which
    /// is known here, before anything is decoded.
    pub fn with_options(mut src: R, opts: XzOptions) -> XzResult<Self> {
        let streams = map_file(&mut src, opts.memory_limit)?;
        let blocks = plan_blocks(&streams, &opts)?;

        let largest_out = blocks
            .iter()
            .map(|b| b.unpacked_len as u64)
            .max()
            .unwrap_or(0);
        let largest_in = blocks.iter().map(|b| b.padded_size).max().unwrap_or(0);
        let per_worker = largest_out
            .checked_add(largest_in)
            .and_then(|v| v.checked_add(PROBS_BYTES))
            .ok_or_else(|| XzError::at(XzErrorKind::SizeMismatch, 0, 0))?;
        if per_worker > opts.memory_limit {
            return Err(XzError::at(
                XzErrorKind::MemoryLimit {
                    needed: per_worker,
                    limit: opts.memory_limit,
                },
                0,
                0,
            ));
        }

        let want = if opts.threads == 0 {
            std::thread::available_parallelism().map_or(1, |n| n.get())
        } else {
            opts.threads
        };
        // Degrade rather than fail: the decode runs with as many workers as
        // the limit pays for, down to one.
        let affordable =
            usize::try_from(opts.memory_limit / per_worker.max(1)).unwrap_or(usize::MAX);
        let threads = want.min(affordable).min(blocks.len()).max(1);

        Ok(XzParallelReader {
            src,
            opts,
            blocks,
            pos: u64::MAX,
            next_dispatch: 0,
            next_emit: 0,
            pool: XzPool::new(),
            threads,
            in_flight: 0,
            ready: BTreeMap::new(),
            current: None,
            spare_out: Vec::new(),
            spare_in: Vec::new(),
            checks: Vec::new(),
            total_out: 0,
            per_worker,
            failed: false,
        })
    }

    /// How many worker threads this decode will use, after degrading to fit
    /// the memory limit.
    #[must_use]
    pub fn threads(&self) -> usize {
        self.threads
    }

    /// What the decode will cost at its peak, in bytes: one block's output
    /// buffer, one block's compressed bytes and one probability table per
    /// worker, sized by the largest block in the file.
    #[must_use]
    pub fn memory_estimate(&self) -> u64 {
        self.per_worker.saturating_mul(self.threads as u64)
    }

    /// How many blocks the file has, across every stream.
    #[must_use]
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// What the index says the file decodes to.
    #[must_use]
    pub fn uncompressed_size(&self) -> u64 {
        self.blocks.iter().map(|b| b.unpacked_len as u64).sum()
    }

    /// The per-block checks computed so far, in block order. Only collected
    /// when the options carry a [`crate::ChecksumPlan`].
    #[must_use]
    pub fn block_checks(&self) -> &[BlockCheck] {
        &self.checks
    }

    /// Gives the underlying source back.
    pub fn into_inner(self) -> R {
        self.src
    }

    /// Reads one block's bytes and hands them to a worker.
    fn dispatch_one(&mut self) -> io::Result<()> {
        let plan = self.blocks[self.next_dispatch];
        if self.pos != plan.file_offset {
            self.src.seek(SeekFrom::Start(plan.file_offset))?;
            self.pos = plan.file_offset;
        }
        let want = usize::try_from(plan.padded_size).map_err(|_| {
            io::Error::from(XzError::at(
                XzErrorKind::SizeMismatch,
                plan.stream,
                plan.file_offset,
            ))
        })?;
        let mut bytes = self.spare_in.pop().unwrap_or_default();
        bytes.clear();
        bytes.try_reserve(want).map_err(|_| {
            io::Error::from(XzError::at(
                XzErrorKind::MemoryLimit {
                    needed: plan.padded_size,
                    limit: self.opts.memory_limit,
                },
                plan.stream,
                plan.file_offset,
            ))
        })?;
        bytes.resize(want, 0u8);
        self.src.read_exact(&mut bytes)?;
        self.pos += plan.padded_size;

        // Grow the pool only as far as there is work for it.
        if self.pool.spawned() < self.threads {
            self.pool.grow();
        }
        let job = XzJob {
            index: self.next_dispatch as u64,
            stream: plan.stream,
            block_in_stream: plan.block_in_stream,
            file_offset: plan.file_offset,
            unpacked_offset: plan.unpacked_offset,
            unpacked_len: plan.unpacked_len,
            unpadded_size: plan.unpadded_size,
            bytes,
            check: plan.check,
            verify: self.opts.verify_checks,
            plan: self.opts.plan.clone(),
            out: self.spare_out.pop().unwrap_or_default(),
        };
        self.pool
            .dispatch(job)
            .map_err(|k| io::Error::from(XzError::at(k, plan.stream, plan.file_offset)))?;
        self.next_dispatch += 1;
        self.in_flight += 1;
        Ok(())
    }

    /// Makes sure the next block in output order is in hand.
    fn advance(&mut self) -> io::Result<bool> {
        loop {
            if let Some(done) = self.ready.remove(&self.next_emit) {
                return self.adopt(done).map(|()| true);
            }
            while self.in_flight < self.threads && self.next_dispatch < self.blocks.len() {
                self.dispatch_one()?;
            }
            if self.in_flight == 0 {
                // Nothing outstanding and nothing left to send: the file is
                // decoded.
                return Ok(false);
            }
            let Some(done) = self.pool.collect() else {
                return Err(io::Error::from(XzError::at(
                    XzErrorKind::Lzma(crate::Error::InternalFailure),
                    0,
                    0,
                )));
            };
            self.in_flight -= 1;
            self.ready.insert(done.index, done);
        }
    }

    /// Takes a finished block as the current output, or reports why it failed.
    fn adopt(&mut self, done: XzDone) -> io::Result<()> {
        let XzDone {
            index: _,
            stream,
            block_in_stream,
            file_offset,
            unpacked_len,
            res,
            out,
            bytes,
            checks,
        } = done;
        self.spare_in.push(bytes);
        if let Err(kind) = res {
            self.failed = true;
            self.spare_out.push(out);
            return Err(io::Error::from(XzError::in_block(
                kind,
                stream,
                block_in_stream,
                file_offset,
            )));
        }
        if let Some(c) = checks
            && !self.opts.plan.is_none()
        {
            self.checks.push(c);
        }
        self.next_emit += 1;
        self.current = Some((out, 0, unpacked_len));
        Ok(())
    }
}

impl<R: Read + Seek> Read for XzParallelReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() || self.failed {
            return Ok(0);
        }
        loop {
            if let Some((buf, pos, len)) = self.current.as_mut() {
                if *pos < *len {
                    let n = (*len - *pos).min(out.len());
                    out[..n].copy_from_slice(&buf[*pos..*pos + n]);
                    *pos += n;
                    self.total_out += n as u64;
                    return Ok(n);
                }
                let (buf, _, _) = self.current.take().expect("current");
                self.spare_out.push(buf);
            }
            if !self.advance()? {
                return Ok(0);
            }
        }
    }
}

/// Reads the index of every stream in the file, last stream first, and
/// returns them in file order.
///
/// C: `XzDecMt` calls `Xz_ReadBackward`, which does exactly this walk: strip
/// stream padding, read the footer, read the index, step to the start of the
/// stream, repeat.
fn map_file<R: Read + Seek>(src: &mut R, memory_limit: u64) -> XzResult<Vec<XzStreamIndex>> {
    let len = src
        .seek(SeekFrom::End(0))
        .map_err(|_| XzError::at(XzErrorKind::TruncatedInput, 0, 0))?;
    if len < (STREAM_HEADER_SIZE * 2) as u64 {
        return Err(XzError::at(XzErrorKind::TruncatedInput, 0, len));
    }
    let mut end = len;
    let mut streams: Vec<XzStreamIndex> = Vec::new();
    while end > 0 {
        // Spec §5: stream padding is a whole number of null four-byte groups.
        loop {
            if end < 4 {
                return Err(XzError::at(XzErrorKind::TrailingGarbage, 0, end));
            }
            let mut tail = [0u8; 4];
            src.seek(SeekFrom::Start(end - 4))
                .and_then(|_| src.read_exact(&mut tail))
                .map_err(|_| XzError::at(XzErrorKind::TruncatedInput, 0, end))?;
            if tail == [0, 0, 0, 0] {
                end -= 4;
            } else {
                break;
            }
        }
        let idx = read_stream_index_ending_at(src, end, memory_limit)?;
        end = idx.stream_offset;
        streams.push(idx);
    }
    streams.reverse();
    Ok(streams)
}

/// Turns the indexes into the block list, checking the caller's caps against
/// what the index says before anything is decoded.
fn plan_blocks(streams: &[XzStreamIndex], opts: &XzOptions) -> XzResult<Vec<Plan>> {
    let mut blocks = Vec::new();
    let mut unpacked_offset = 0u64;
    let mut total = 0u64;
    for (stream_no, s) in streams.iter().enumerate() {
        let stream = stream_no as u64;
        let entries = s
            .index
            .blocks(s.stream_offset)
            .ok_or_else(|| XzError::at(XzErrorKind::IndexMismatch, stream, s.stream_offset))?;
        for (i, e) in entries.iter().enumerate() {
            let unpacked_len = usize::try_from(e.record.uncompressed_size).map_err(|_| {
                XzError::at(
                    XzErrorKind::TooMuchOutput { limit: 0 },
                    stream,
                    e.file_offset,
                )
            })?;
            if e.record.uncompressed_size > opts.max_block_size {
                return Err(XzError::in_block(
                    XzErrorKind::TooMuchOutput {
                        limit: opts.max_block_size,
                    },
                    stream,
                    i as u64,
                    e.file_offset,
                ));
            }
            total = total
                .checked_add(e.record.uncompressed_size)
                .ok_or_else(|| XzError::at(XzErrorKind::SizeMismatch, stream, e.file_offset))?;
            if let Some(cap) = opts.max_unpack_bytes
                && total > cap
            {
                return Err(XzError::in_block(
                    XzErrorKind::TooMuchOutput { limit: cap },
                    stream,
                    i as u64,
                    e.file_offset,
                ));
            }
            let padded_size = e
                .record
                .padded_size()
                .ok_or_else(|| XzError::at(XzErrorKind::IndexMismatch, stream, e.file_offset))?;
            blocks.push(Plan {
                stream,
                block_in_stream: i as u64,
                file_offset: e.file_offset,
                unpacked_offset,
                unpacked_len,
                unpadded_size: e.record.unpadded_size,
                padded_size,
                check: s.header.flags.check,
            });
            unpacked_offset += e.record.uncompressed_size;
        }
        // A stream whose check this build cannot compute is refused here for
        // the same reason the sequential reader refuses it: unchecked bytes
        // are never returned by accident.
        let check = s.header.flags.check;
        if opts.verify_checks && !check.is_verifiable() && !opts.allow_unverifiable {
            return Err(XzError::at(
                XzErrorKind::UnsupportedCheck,
                stream,
                s.stream_offset,
            ));
        }
    }
    if blocks.is_empty() {
        return Err(XzError::at(XzErrorKind::IndexMismatch, 0, 0));
    }
    Ok(blocks)
}
