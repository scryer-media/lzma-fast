//! What can go wrong reading an `.xz` file, and where.
//!
//! Every error carries the stream and block it happened in and the file
//! offset, because a caller that has already written some output needs to
//! know how much of it is good. C: `XzDec.c` returns bare `SZ_ERROR_DATA` and
//! leaves the caller to guess; this does not copy that.

use core::fmt;

pub use crate::error::XzErrorKind;

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
