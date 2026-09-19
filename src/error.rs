//! Error, status and progress types.
//!
//! These mirror the reference decoder's `SRes` codes (`SZ_ERROR_DATA`,
//! `SZ_ERROR_UNSUPPORTED`, `SZ_ERROR_FAIL`) and its `ELzmaStatus` /
//! `ELzmaFinishMode` enumerations from `C/LzmaDec.h`.

use core::fmt;

/// Why an xz read failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum XzErrorKind {
    /// The stream header's magic bytes are not `FD 37 7A 58 5A 00`.
    BadMagic,
    /// The footer's magic bytes are not `YZ`.
    BadFooterMagic,
    /// A reserved bit is set in the stream flags, or the first flag byte is
    /// not zero.
    BadStreamFlags,
    /// The stream footer's flags do not match the header's.
    StreamFlagsMismatch,
    /// A CRC-32 over a header, footer or index did not match.
    HeaderCrc,
    /// The stream's check type is one this build cannot compute, and the
    /// caller did not allow an unverified decode.
    UnsupportedCheck,
    /// The check stored for a block does not match what the block decoded to.
    CheckMismatch,
    /// A filter id the crate does not implement.
    UnsupportedFilter,
    /// A filter chain the format does not allow: too many filters, a
    /// non-last filter last, a last filter used as a non-last one, a repeated
    /// filter, or properties of the wrong size.
    BadFilterChain,
    /// A variable-length integer that is too long, too wide, or not the
    /// shortest encoding of its value.
    BadVli,
    /// Input ended inside a variable-length integer.
    TruncatedVli,
    /// A padding field contained something other than null bytes, or was the
    /// wrong length.
    BadPadding,
    /// A block header's size byte, flags or fields are out of range.
    BadBlockHeader,
    /// A size declared in a block header or in the index does not match what
    /// was actually read or produced.
    SizeMismatch,
    /// The index does not describe the blocks that were decoded: the record
    /// count, a record's sizes, or the position the index sits at.
    IndexMismatch,
    /// Input ended before the structure being read was complete.
    TruncatedInput,
    /// Bytes follow the last stream that are neither stream padding nor
    /// another stream.
    TrailingGarbage,
    /// Honouring the stream would need more memory than the caller allowed.
    /// Carries what was needed.
    MemoryLimit {
        /// Bytes the stream asks for.
        needed: u64,
        /// Bytes the caller allowed.
        limit: u64,
    },
    /// The stream decodes to more than the caller's `max_unpack_bytes`.
    TooMuchOutput {
        /// The cap that was exceeded.
        limit: u64,
    },
    /// The LZMA2 layer refused the compressed data.
    Lzma(Error),
}

impl fmt::Display for XzErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            XzErrorKind::BadMagic => f.write_str("not an xz stream"),
            XzErrorKind::BadFooterMagic => f.write_str("bad xz stream footer magic"),
            XzErrorKind::BadStreamFlags => f.write_str("bad xz stream flags"),
            XzErrorKind::StreamFlagsMismatch => {
                f.write_str("xz stream footer flags differ from the header's")
            }
            XzErrorKind::HeaderCrc => f.write_str("xz header CRC-32 mismatch"),
            XzErrorKind::UnsupportedCheck => f.write_str("unsupported xz check type"),
            XzErrorKind::CheckMismatch => f.write_str("xz block check mismatch"),
            XzErrorKind::UnsupportedFilter => f.write_str("unsupported xz filter"),
            XzErrorKind::BadFilterChain => f.write_str("invalid xz filter chain"),
            XzErrorKind::BadVli => f.write_str("invalid xz variable-length integer"),
            XzErrorKind::TruncatedVli => {
                f.write_str("xz input ended inside a variable-length integer")
            }
            XzErrorKind::BadPadding => f.write_str("xz padding is not null bytes"),
            XzErrorKind::BadBlockHeader => f.write_str("invalid xz block header"),
            XzErrorKind::SizeMismatch => f.write_str("xz size field does not match the data"),
            XzErrorKind::IndexMismatch => f.write_str("xz index does not match the blocks"),
            XzErrorKind::TruncatedInput => f.write_str("truncated xz input"),
            XzErrorKind::TrailingGarbage => f.write_str("trailing garbage after the xz stream"),
            XzErrorKind::MemoryLimit { needed, limit } => write!(
                f,
                "xz stream needs {needed} bytes, over the {limit}-byte limit"
            ),
            XzErrorKind::TooMuchOutput { limit } => {
                write!(f, "xz stream decodes to more than the {limit}-byte cap")
            }
            XzErrorKind::Lzma(e) => write!(f, "{e}"),
        }
    }
}

impl core::error::Error for XzErrorKind {}

impl From<Error> for XzErrorKind {
    fn from(e: Error) -> Self {
        XzErrorKind::Lzma(e)
    }
}

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
