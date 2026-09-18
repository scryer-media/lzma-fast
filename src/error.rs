//! Error, status and progress types.
//!
//! These mirror the reference decoder's `SRes` codes (`SZ_ERROR_DATA`,
//! `SZ_ERROR_UNSUPPORTED`, `SZ_ERROR_FAIL`) and its `ELzmaStatus` /
//! `ELzmaFinishMode` enumerations from `C/LzmaDec.h`.

use core::fmt;

/// Reasons a decode call can fail.
///
/// C: the `SZ_ERROR_*` subset that `LzmaDec.c` and `Lzma2Dec.c` can return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// C: `SZ_ERROR_UNSUPPORTED`. The property byte(s) do not describe a
    /// stream this decoder can handle.
    UnsupportedProps,
    /// C: `SZ_ERROR_DATA`. The compressed stream is malformed: a match refers
    /// outside the dictionary, a range coder check failed, the end marker was
    /// absent where one was required, or an LZMA2 chunk header is invalid.
    CorruptData,
    /// C: `SZ_ERROR_FAIL`. Reached only if the decoder's own invariants are
    /// violated; it is never produced by malformed input alone.
    InternalFailure,
    /// The decoder could not allocate its dictionary or probability table.
    Alloc,
    /// [`Error::CorruptData`], located.
    ///
    /// Only the multi-threaded decoder produces this: a worker decodes one
    /// independently decodable run and so can say which one failed and where
    /// its output would have gone, which a caller writing blocks by offset
    /// needs in order to know what it has and has not got.
    CorruptRun {
        /// The run's index in the stream, counting from zero.
        index: u64,
        /// Where the run's output starts, in bytes from the start of the
        /// decoded stream.
        out_offset: u64,
    },
    /// The decode was cancelled by the caller.
    Cancelled,
    /// C: `SZ_ERROR_PARAM`. An encoder setting is out of range.
    Param,
    /// C: `SZ_ERROR_READ`. The encoder's input stream failed.
    Read,
    /// C: `SZ_ERROR_WRITE`. The encoder's output stream failed.
    Write,
    /// C: `SZ_ERROR_OUTPUT_EOF`. The encoder's output buffer is too small for
    /// the data it was asked to produce.
    OutputEof,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Error::UnsupportedProps => "unsupported LZMA properties",
            Error::CorruptData => "corrupt LZMA data",
            Error::InternalFailure => "internal LZMA decoder failure",
            Error::Alloc => "LZMA decoder allocation failed",
            Error::CorruptRun { index, out_offset } => {
                return write!(
                    f,
                    "corrupt LZMA2 data in run {index}, at output offset {out_offset}"
                );
            }
            Error::Cancelled => "LZMA decode cancelled",
            Error::Param => "invalid LZMA encoder parameter",
            Error::Read => "LZMA encoder input stream failed",
            Error::Write => "LZMA encoder output stream failed",
            Error::OutputEof => "LZMA encoder output buffer too small",
        };
        f.write_str(s)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

/// Where a decode call stopped.
///
/// C: `ELzmaStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Status {
    /// C: `LZMA_STATUS_NOT_SPECIFIED`. No useful status; the caller should use
    /// the returned counts instead.
    NotSpecified,
    /// C: `LZMA_STATUS_FINISHED_WITH_MARK`. The stream ended with an end
    /// marker.
    FinishedWithMark,
    /// C: `LZMA_STATUS_NOT_FINISHED`. The output limit was reached and the
    /// stream continues.
    NotFinished,
    /// C: `LZMA_STATUS_NEEDS_MORE_INPUT`. All input was consumed mid-symbol.
    NeedsMoreInput,
    /// C: `LZMA_STATUS_MAYBE_FINISHED_WITHOUT_MARK`. The output limit was
    /// reached with the range coder in a state that could be a clean end of a
    /// stream that carries no end marker.
    MaybeFinishedWithoutMark,
}

/// Whether the current output buffer is expected to cover the end of the
/// stream.
///
/// C: `ELzmaFinishMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishMode {
    /// C: `LZMA_FINISH_ANY`. Decode at most the bytes that fit; do not look
    /// ahead for an end marker.
    Any,
    /// C: `LZMA_FINISH_END`. The block must be finished when the output limit
    /// is reached.
    End,
}

/// What one decode call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Bytes consumed from the input slice.
    pub read: usize,
    /// Bytes produced into the output slice.
    pub written: usize,
    /// Where the call stopped.
    pub status: Status,
}
