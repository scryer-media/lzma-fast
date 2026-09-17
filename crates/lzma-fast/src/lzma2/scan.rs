//! Run-boundary discovery: where an LZMA2 stream can be cut.
//!
//! An LZMA2 stream is a series of chunks, and a chunk whose control byte asks
//! for a dictionary reset starts a *run* that decodes without reference to
//! anything before it. Runs are the only parallelism the format has, and they
//! are also the only points at which a decode may be handed from one decoder
//! to another, so both the multi-threaded decoder and a caller deciding how to
//! schedule work need the same answer to "where are they?".
//!
//! This is that answer, and it is cheap: it reads chunk headers and skips
//! chunk payloads, so it costs O(chunks), never decodes, and holds a fixed
//! amount of state no matter how the input is split across calls. It runs the
//! same [`Lzma2Frame`] header state machine as the decoder itself, so it
//! cannot disagree with it about what a control byte means.
//!
//! C: there is no counterpart. `Lzma2Dec_Parse` finds the same boundaries, but
//! only as a side effect of filling a worker's block, and its answer is not
//! visible to the caller.

use alloc::collections::VecDeque;

use crate::error::Error;
use crate::lzma::LzmaProps;
use crate::lzma2::frame::{
    LZMA2_CONTROL_COPY_RESET_DIC, Lzma2Frame, Lzma2State, is_uncompressed_state,
};

/// One independently decodable region of an LZMA2 stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lzma2Run {
    /// Offset of the run's first control byte, from the start of the stream.
    pub in_offset: u64,
    /// The run's length in the compressed stream.
    pub packed_len: u64,
    /// Offset of the run's first decoded byte in the output.
    pub out_offset: u64,
    /// How many bytes the run's chunk headers say it decodes to.
    pub unpacked_len: u64,
    /// Whether the run begins with a dictionary reset, and so is genuinely
    /// independent.
    ///
    /// False only for the first run of a stream that does not begin with one,
    /// which is malformed but is reported rather than rejected here: the
    /// decoder is the thing that decides a stream is bad.
    pub has_dict_reset: bool,
}

/// Incremental discovery of the runs in an LZMA2 stream.
///
/// Feed it bytes as they arrive with [`Lzma2RunScanner::feed`] and take
/// finished runs out with [`Lzma2RunScanner::next_run`]. A run is reported
/// once the control byte *after* it has been seen, or once the stream's end
/// marker has: until then its length is not yet known.
#[derive(Debug, Clone)]
pub struct Lzma2RunScanner {
    frame: Lzma2Frame,
    /// Scratch for [`Lzma2State::Prop`], which the shared state machine writes
    /// through. The scanner never decodes, so the values are not used.
    prop: LzmaProps,
    in_pos: u64,
    out_pos: u64,
    /// Bytes of the current chunk's payload still to be skipped.
    data_remaining: u64,
    open: Option<OpenRun>,
    ready: VecDeque<Lzma2Run>,
    finished: bool,
    failed: bool,
}

#[derive(Debug, Clone, Copy)]
struct OpenRun {
    in_offset: u64,
    out_offset: u64,
    has_dict_reset: bool,
}

impl Default for Lzma2RunScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Lzma2RunScanner {
    /// A scanner positioned at the start of a stream.
    #[must_use]
    pub fn new() -> Self {
        Lzma2RunScanner {
            frame: Lzma2Frame::new(),
            // Any valid triple will do; the scanner does not decode.
            prop: LzmaProps::new(0, 0, 0, 1 << 12).expect("0/0/0 is in range"),
            in_pos: 0,
            out_pos: 0,
            data_remaining: 0,
            open: None,
            ready: VecDeque::new(),
            finished: false,
            failed: false,
        }
    }

    /// True once the stream's end marker has been read. No further input will
    /// be consumed.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.finished
    }

    /// How many bytes of the stream have been scanned.
    #[must_use]
    pub fn in_position(&self) -> u64 {
        self.in_pos
    }

    /// How many bytes the chunk headers seen so far account for.
    #[must_use]
    pub fn out_position(&self) -> u64 {
        self.out_pos
    }

    /// How many complete runs are waiting to be taken.
    #[must_use]
    pub fn pending_runs(&self) -> usize {
        self.ready.len()
    }

    /// Where the run currently being scanned starts, if there is one.
    ///
    /// A caller keeping fed bytes around for a decoder needs this: nothing
    /// before it will be asked for again.
    #[must_use]
    pub fn open_run_offset(&self) -> Option<u64> {
        self.open.map(|o| o.in_offset)
    }

    /// Bytes of the current chunk's payload still to be walked past.
    ///
    /// The scanner never looks at payload bytes, only at how many there are,
    /// so a caller reading from a seekable source can skip them with a seek
    /// instead of a read: ask this, seek that far, and say so with
    /// [`Lzma2RunScanner::skip_payload`].
    #[must_use]
    pub fn payload_remaining(&self) -> u64 {
        self.data_remaining
    }

    /// Tells the scanner that `n` bytes of the current chunk's payload were
    /// skipped rather than fed, and returns how many it accepted.
    ///
    /// Never more than [`Lzma2RunScanner::payload_remaining`]; a caller that
    /// skipped further than that has skipped a chunk header, which the scanner
    /// cannot recover from and will not pretend to.
    pub fn skip_payload(&mut self, n: u64) -> u64 {
        let take = n.min(self.data_remaining);
        self.in_pos += take;
        self.data_remaining -= take;
        take
    }

    /// Takes the oldest complete run.
    pub fn next_run(&mut self) -> Option<Lzma2Run> {
        self.ready.pop_front()
    }

    /// Scans `data`, which continues the stream where the last call left off,
    /// and returns how many of its bytes were consumed.
    ///
    /// Consumes everything unless the stream ends inside `data`. Chunk headers
    /// split across calls are carried over, never re-read.
    ///
    /// # Errors
    ///
    /// Returns [`Error::CorruptData`] for a control byte the format does not
    /// allow in that position. A scanner that has returned an error will keep
    /// returning it.
    pub fn feed(&mut self, data: &[u8]) -> Result<usize, Error> {
        if self.failed {
            return Err(Error::CorruptData);
        }
        let mut pos = 0usize;
        while pos < data.len() {
            if self.finished {
                break;
            }
            if self.data_remaining != 0 {
                let want = usize::try_from(self.data_remaining)
                    .unwrap_or(usize::MAX)
                    .min(data.len() - pos);
                pos += want;
                self.in_pos += want as u64;
                self.data_remaining -= want as u64;
                continue;
            }

            let b = data[pos];
            if self.frame.state == Lzma2State::Control && !self.on_control(b) {
                pos += 1;
                self.in_pos += 1;
                break;
            }

            let next = self.frame.update_state(b, &mut self.prop);
            pos += 1;
            self.in_pos += 1;
            self.frame.state = next;

            match next {
                Lzma2State::Error => {
                    self.failed = true;
                    return Err(Error::CorruptData);
                }
                Lzma2State::Data => {
                    self.out_pos += u64::from(self.frame.unpack_size);
                    self.data_remaining = if is_uncompressed_state(self.frame.control) {
                        u64::from(self.frame.unpack_size)
                    } else {
                        u64::from(self.frame.pack_size)
                    };
                    // The decoder decrements `unpackSize` as it produces the
                    // chunk's output, so by the time the next control byte
                    // arrives it is zero, and `LZMA2_STATE_UNPACK0` ORs into
                    // it rather than assigning. Skipping the payload has to
                    // leave the same invariant behind.
                    self.frame.unpack_size = 0;
                    // The header state machine expects the payload to be
                    // consumed and the next byte to be a control byte again.
                    self.frame.state = Lzma2State::Control;
                }
                _ => {}
            }
        }
        Ok(pos)
    }

    /// Handles a control byte before the state machine sees it. Returns false
    /// if it ends the stream.
    fn on_control(&mut self, b: u8) -> bool {
        let starts_run = b == 0 || b >= 0xE0 || b == LZMA2_CONTROL_COPY_RESET_DIC;
        if starts_run {
            self.close_run();
        }
        if b == 0 {
            self.finished = true;
            self.frame.state = Lzma2State::Finished;
            return false;
        }
        if self.open.is_none() {
            self.open = Some(OpenRun {
                in_offset: self.in_pos,
                out_offset: self.out_pos,
                has_dict_reset: starts_run,
            });
        }
        true
    }

    fn close_run(&mut self) {
        if let Some(o) = self.open.take() {
            self.ready.push_back(Lzma2Run {
                in_offset: o.in_offset,
                packed_len: self.in_pos - o.in_offset,
                out_offset: o.out_offset,
                unpacked_len: self.out_pos - o.out_offset,
                has_dict_reset: o.has_dict_reset,
            });
        }
    }
}

/// The runs of an LZMA2 stream in a seekable source, without decoding it.
///
/// Scans from the source's current position to the stream's end marker,
/// seeking past chunk payloads rather than reading them, so the cost is one
/// small read per chunk header and not one pass over the packed bytes. The
/// source's position is restored before returning, so a caller can ask this
/// about a packed range and then decode it.
///
/// This is the question a consumer asks before it commits: how many
/// independently decodable runs are in this range, how big are they, and is
/// there enough there to be worth widening for. [`Lzma2RunScanner`] answers it
/// for bytes as they arrive; this answers it for bytes already on disk.
///
/// `dict_prop` is validated and otherwise unused: finding run boundaries needs
/// no dictionary. It is in the signature so that a caller passing a property
/// byte no decoder would accept finds out here rather than after it has
/// committed to a decode.
///
/// # Errors
///
/// [`std::io::ErrorKind::InvalidInput`] for `dict_prop > 40`,
/// [`std::io::ErrorKind::InvalidData`] for a stream the scanner rejects,
/// [`std::io::ErrorKind::UnexpectedEof`] for a range that ends before the
/// stream's end marker does, and any error the source itself returns.
#[cfg(feature = "std")]
pub fn run_boundaries<R: std::io::Read + std::io::Seek>(
    mut source: R,
    dict_prop: u8,
) -> std::io::Result<Vec<Lzma2Run>> {
    use std::io::{Error as IoError, ErrorKind, SeekFrom};

    if dict_prop > 40 {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            Error::UnsupportedProps,
        ));
    }
    let start = source.stream_position()?;
    let end = source.seek(SeekFrom::End(0))?;
    source.seek(SeekFrom::Start(start))?;

    let mut scanner = Lzma2RunScanner::new();
    let mut runs = Vec::new();
    let mut buf = [0u8; 4096];
    let mut at = start;
    let mut failed = None;

    while !scanner.finished() {
        while let Some(r) = scanner.next_run() {
            runs.push(r);
        }
        let skip = scanner.payload_remaining();
        if skip != 0 {
            if skip > end - at {
                failed = Some(IoError::new(ErrorKind::UnexpectedEof, Error::CorruptData));
                break;
            }
            scanner.skip_payload(skip);
            at += skip;
            source.seek(SeekFrom::Start(at))?;
            continue;
        }
        let want = usize::try_from(end - at)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        if want == 0 {
            failed = Some(IoError::new(ErrorKind::UnexpectedEof, Error::CorruptData));
            break;
        }
        let n = source.read(&mut buf[..want])?;
        if n == 0 {
            failed = Some(IoError::new(ErrorKind::UnexpectedEof, Error::CorruptData));
            break;
        }
        match scanner.feed(&buf[..n]) {
            Ok(used) => {
                at += used as u64;
                if used != n {
                    source.seek(SeekFrom::Start(at))?;
                }
            }
            Err(e) => {
                failed = Some(IoError::new(ErrorKind::InvalidData, e));
                break;
            }
        }
    }
    while let Some(r) = scanner.next_run() {
        runs.push(r);
    }
    source.seek(SeekFrom::Start(start))?;
    match failed {
        Some(e) => Err(e),
        None => Ok(runs),
    }
}
