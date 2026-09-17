//! What can go wrong reading an `.xz` file, and where.
//!
//! Every error carries the stream and block it happened in and the file
//! offset, because a caller that has already written some output needs to
//! know how much of it is good. C: `XzDec.c` returns bare `SZ_ERROR_DATA` and
//! leaves the caller to guess; this does not copy that.

use core::fmt;

use crate::error::Error;

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

/// An xz read failure, located.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XzError {
    /// What went wrong.
    pub kind: XzErrorKind,
    /// Which stream of a concatenated file, counting from zero.
    pub stream: u64,
    /// Which block of that stream, counting from zero, if the failure
    /// belongs to one.
    pub block: Option<u64>,
    /// Offset in the file where the failure was noticed.
    pub offset: u64,
}

impl XzError {
    /// An error at `offset` in stream `stream`, outside any block.
    #[must_use]
    pub fn at(kind: impl Into<XzErrorKind>, stream: u64, offset: u64) -> Self {
        XzError {
            kind: kind.into(),
            stream,
            block: None,
            offset,
        }
    }

    /// An error inside block `block` of stream `stream`.
    #[must_use]
    pub fn in_block(kind: impl Into<XzErrorKind>, stream: u64, block: u64, offset: u64) -> Self {
        XzError {
            kind: kind.into(),
            stream,
            block: Some(block),
            offset,
        }
    }
}

impl fmt::Display for XzError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)?;
        match self.block {
            Some(b) => write!(
                f,
                " (stream {}, block {}, file offset {})",
                self.stream, b, self.offset
            ),
            None => write!(f, " (stream {}, file offset {})", self.stream, self.offset),
        }
    }
}

impl core::error::Error for XzError {}

impl From<XzError> for std::io::Error {
    fn from(e: XzError) -> Self {
        let kind = match e.kind {
            XzErrorKind::TruncatedInput | XzErrorKind::TruncatedVli => {
                std::io::ErrorKind::UnexpectedEof
            }
            XzErrorKind::MemoryLimit { .. } => std::io::ErrorKind::OutOfMemory,
            XzErrorKind::UnsupportedCheck | XzErrorKind::UnsupportedFilter => {
                std::io::ErrorKind::Unsupported
            }
            _ => std::io::ErrorKind::InvalidData,
        };
        std::io::Error::new(kind, e)
    }
}

/// The result type the xz layer works in.
pub type XzResult<T> = Result<T, XzError>;
