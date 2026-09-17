//! Decoding one block: LZMA2, then the chain's converters, then the check.
//!
//! C: `XzDec_DecodeBlock` and the `CXzUnpacker` filter loop in `XzDec.c`,
//! collapsed into one object because this crate has exactly one last-filter.
//!
//! Two paths, and the fast one is the point: a block whose chain is a bare
//! LZMA2 filter decodes straight into the caller's buffer, with the check
//! computed over that buffer and nothing copied. Only a chain with converters
//! in it pays for an intermediate buffer, because a converter has to see its
//! bytes twice.

use alloc::vec::Vec;

use super::check::{BlockCheck, RunningCheck};
use super::error::XzErrorKind;
use super::filter::{Converters, FilterChain};
use super::stream::CheckType;
use crate::error::{FinishMode, Status};
use crate::lzma2::Lzma2Decoder;
use crate::mt::checksum::ChecksumPlan;

/// How much LZMA2 output is produced per turn on the buffered path.
const SCRATCH: usize = 1 << 16;

/// What one block decode is allowed to cost and to produce.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BlockLimits {
    /// Bytes the decoder may allocate for this block.
    pub(crate) memory_limit: u64,
    /// The uncompressed size the header declares, if any. Used both to check
    /// the block and to shrink the dictionary.
    pub(crate) uncompressed_size: Option<u64>,
    /// The compressed size the header declares, if any.
    pub(crate) compressed_size: Option<u64>,
    /// A cap on the block's output whether or not the header declares one.
    pub(crate) max_block_size: u64,
}

/// One block, decoding.
pub(crate) struct BlockDecoder {
    lzma2: Lzma2Decoder,
    converters: Converters,
    check: RunningCheck,
    limits: BlockLimits,
    /// Output produced by the converters and not yet handed to the caller.
    pending: Vec<u8>,
    pending_pos: usize,
    scratch: Vec<u8>,
    unpacked: u64,
    packed: u64,
    /// True once LZMA2 has reached its end marker and the converters have
    /// been flushed.
    finished: bool,
}

/// The dictionary a block needs, in bytes, after clamping to what it can
/// possibly use.
///
/// The dictionary never has to be bigger than the block's own output: xz
/// resets the dictionary at every block, so no match can reach further back
/// than the block's first byte. When the header declares an uncompressed size
/// this turns a 64 MiB `xz -9` dictionary into a few kilobytes for a small
/// block, which is the difference between fitting a caller's limit and not.
pub(crate) fn dict_bytes(dict_prop: u8, uncompressed_size: Option<u64>) -> u64 {
    let declared = u64::from(crate::lzma2::frame::dic_size_from_prop_full(dict_prop));
    match uncompressed_size {
        Some(n) => declared.min(n.max(u64::from(crate::lzma::consts::LZMA_DIC_MIN))),
        None => declared,
    }
}

impl BlockDecoder {
    /// Builds a decoder for one block.
    ///
    /// # Errors
    ///
    /// [`XzErrorKind::MemoryLimit`] if the dictionary does not fit,
    /// [`XzErrorKind::TooMuchOutput`] if the header declares more output than
    /// `max_block_size`, and the LZMA2 errors for an impossible property byte.
    pub(crate) fn new(
        chain: &FilterChain,
        check: CheckType,
        unpacked_offset: u64,
        plan: &ChecksumPlan,
        verify: bool,
        limits: BlockLimits,
    ) -> Result<Self, XzErrorKind> {
        if let Some(n) = limits.uncompressed_size
            && n > limits.max_block_size
        {
            return Err(XzErrorKind::TooMuchOutput {
                limit: limits.max_block_size,
            });
        }
        let dict = dict_bytes(chain.dict_prop, limits.uncompressed_size);
        if dict > limits.memory_limit {
            return Err(XzErrorKind::MemoryLimit {
                needed: dict,
                limit: limits.memory_limit,
            });
        }
        let cap = u32::try_from(dict).unwrap_or(u32::MAX);
        let lzma2 = Lzma2Decoder::new_capped(chain.dict_prop, cap)?;
        Ok(BlockDecoder {
            lzma2,
            converters: chain.build()?,
            check: RunningCheck::new(check, unpacked_offset, plan, verify),
            limits,
            pending: Vec::new(),
            pending_pos: 0,
            scratch: Vec::new(),
            unpacked: 0,
            packed: 0,
            finished: false,
        })
    }

    /// Bytes of compressed data consumed so far.
    pub(crate) fn packed(&self) -> u64 {
        self.packed
    }

    /// Bytes of output produced so far.
    pub(crate) fn unpacked(&self) -> u64 {
        self.unpacked
    }

    /// True once the block's end marker has been read and everything the
    /// converters held has been handed over.
    pub(crate) fn finished(&self) -> bool {
        self.finished && self.pending_pos == self.pending.len()
    }

    /// Decodes into `out`, taking what it needs from `input`.
    ///
    /// Returns `(input consumed, output written)`. A return of `(0, 0)` with
    /// `finished()` false means more input is needed.
    ///
    /// # Errors
    ///
    /// The LZMA2 errors, [`XzErrorKind::TooMuchOutput`] if the block runs past
    /// its declared or configured size, and [`XzErrorKind::SizeMismatch`] if it
    /// ends somewhere else than where the header said it would.
    pub(crate) fn decode(
        &mut self,
        input: &[u8],
        out: &mut [u8],
    ) -> Result<(usize, usize), XzErrorKind> {
        // Whatever the converters already produced goes first.
        if self.pending_pos < self.pending.len() {
            let n = (self.pending.len() - self.pending_pos).min(out.len());
            out[..n].copy_from_slice(&self.pending[self.pending_pos..self.pending_pos + n]);
            self.pending_pos += n;
            if self.pending_pos == self.pending.len() {
                self.pending.clear();
                self.pending_pos = 0;
            }
            return Ok((0, n));
        }
        if self.finished || out.is_empty() {
            return Ok((0, 0));
        }

        // Never read past the declared compressed size: the bytes after it are
        // padding and the check, not compressed data.
        let input = match self.limits.compressed_size {
            Some(c) => {
                // The end marker is inside the compressed size, so a block
                // that has consumed all of it without reaching the marker is
                // not the block its header describes. Saying so here matters:
                // the caller cannot see that the input it still holds is out
                // of bounds for this block, so it would otherwise offer those
                // bytes forever and never be told they cannot be used.
                if self.packed >= c {
                    return Err(XzErrorKind::SizeMismatch);
                }
                let left = usize::try_from(c - self.packed).unwrap_or(usize::MAX);
                &input[..input.len().min(left)]
            }
            None => input,
        };

        if self.converters.is_empty() {
            // The fast path: LZMA2 writes into the caller's buffer.
            let limit = self.room(out.len())?;
            // A block whose declared size has been produced still owes its
            // end-of-stream control byte, so LZMA2 is called once more with
            // no room and `FinishMode::End` to read it. C: `XzUnpacker_Code`
            // calls the coder with `outSize == 0` for exactly this.
            let finish = if limit == 0 {
                FinishMode::End
            } else {
                FinishMode::Any
            };
            let p = self.decode_lzma2(input, &mut out[..limit], finish)?;
            self.packed += p.read as u64;
            self.unpacked += p.written as u64;
            self.check.update(&out[..p.written]);
            self.note(p.status)?;
            if limit == 0 && !self.finished {
                return Err(self.at_capacity());
            }
            return Ok((p.read, p.written));
        }

        // The buffered path: LZMA2 into scratch, scratch through the
        // converters, converted bytes into `pending`.
        let want = self.room(SCRATCH)?;
        let finish = if want == 0 {
            FinishMode::End
        } else {
            FinishMode::Any
        };
        if self.scratch.len() < want {
            self.scratch.resize(want, 0u8);
        }
        let mut scratch = core::mem::take(&mut self.scratch);
        let decoded = self.decode_lzma2(input, &mut scratch[..want], finish);
        self.scratch = scratch;
        let p = decoded?;
        self.packed += p.read as u64;
        let produced = &self.scratch[..p.written];
        if p.status == Status::FinishedWithMark {
            self.converters.finish(produced, &mut self.pending);
        } else {
            self.converters.push(produced, &mut self.pending);
        }
        self.unpacked += self.pending.len() as u64;
        self.check.update(&self.pending);
        self.note(p.status)?;
        if want == 0 && !self.finished {
            return Err(self.at_capacity());
        }

        let n = self.pending.len().min(out.len());
        out[..n].copy_from_slice(&self.pending[..n]);
        self.pending_pos = n;
        if n == self.pending.len() {
            self.pending.clear();
            self.pending_pos = 0;
        }
        Ok((p.read, n))
    }

    /// One LZMA2 call, with its failure read for what it means here.
    ///
    /// With no room left the call exists only to read the end marker, and
    /// LZMA2 fails it when what follows is another chunk. That is a block
    /// running past its allowance, not corrupt data: reported as corruption, a
    /// caller that hit its own cap is told its file is broken.
    fn decode_lzma2(
        &mut self,
        input: &[u8],
        out: &mut [u8],
        finish: FinishMode,
    ) -> Result<crate::error::Progress, XzErrorKind> {
        let full = out.is_empty();
        self.lzma2.decode(input, out, finish).map_err(|error| {
            if full {
                self.at_capacity()
            } else {
                error.into()
            }
        })
    }

    /// The block has produced everything it is allowed to and has not stopped.
    fn at_capacity(&self) -> XzErrorKind {
        match self.limits.uncompressed_size {
            // The header said how big it was and it is bigger.
            Some(_) => XzErrorKind::SizeMismatch,
            // No declared size, so it is the caller's cap that ran out.
            None => XzErrorKind::TooMuchOutput {
                limit: self.limits.max_block_size,
            },
        }
    }

    /// How much output may still be produced, capped at `want`.
    fn room(&self, want: usize) -> Result<usize, XzErrorKind> {
        let cap = match self.limits.uncompressed_size {
            Some(n) => n,
            None => self.limits.max_block_size,
        };
        if self.unpacked > cap {
            return Err(XzErrorKind::TooMuchOutput { limit: cap });
        }
        Ok(usize::try_from(cap - self.unpacked)
            .unwrap_or(usize::MAX)
            .min(want))
    }

    fn note(&mut self, status: Status) -> Result<(), XzErrorKind> {
        if status == Status::FinishedWithMark {
            self.finished = true;
            if let Some(n) = self.limits.uncompressed_size
                && n != self.unpacked
            {
                return Err(XzErrorKind::SizeMismatch);
            }
            if let Some(c) = self.limits.compressed_size
                && c != self.packed
            {
                return Err(XzErrorKind::SizeMismatch);
            }
        }
        Ok(())
    }

    /// Closes the block against the `stored` bytes of its check field.
    ///
    /// # Errors
    ///
    /// [`XzErrorKind::CheckMismatch`] if what was decoded does not match.
    pub(crate) fn finish(self, stored: &[u8]) -> Result<BlockCheck, XzErrorKind> {
        self.check.finish(stored)
    }
}
