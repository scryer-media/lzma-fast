//! The `.lzma` (LZMA-alone) container header.
//!
//! C: `C/Util/Lzma/LzmaUtil.c`, which reads a 13-byte header of 5 property
//! bytes followed by a little-endian 64-bit uncompressed size, where
//! `0xFFFF_FFFF_FFFF_FFFF` means "unknown, the stream carries an end marker".

use crate::error::Error;
use crate::lzma::LzmaProps;
use crate::lzma::consts::LZMA_PROPS_SIZE;

/// Size of a `.lzma` header in bytes.
pub const LZMA_ALONE_HEADER_SIZE: usize = LZMA_PROPS_SIZE + 8;

/// A parsed `.lzma` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LzmaAloneHeader {
    /// The LZMA properties from the first 5 bytes.
    pub props: LzmaProps,
    /// The declared uncompressed size, or `None` when the header records the
    /// unknown-size sentinel and the stream ends with an end marker.
    pub uncompressed_size: Option<u64>,
}

impl LzmaAloneHeader {
    /// Parses a 13-byte `.lzma` header.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedProps`] if the property byte is out of
    /// range.
    pub fn parse(header: &[u8; LZMA_ALONE_HEADER_SIZE]) -> Result<Self, Error> {
        let mut props = [0u8; LZMA_PROPS_SIZE];
        props.copy_from_slice(&header[..LZMA_PROPS_SIZE]);
        let props = LzmaProps::parse(&props)?;

        let mut size_bytes = [0u8; 8];
        size_bytes.copy_from_slice(&header[LZMA_PROPS_SIZE..]);
        let raw = u64::from_le_bytes(size_bytes);
        let uncompressed_size = if raw == u64::MAX { None } else { Some(raw) };

        Ok(LzmaAloneHeader {
            props,
            uncompressed_size,
        })
    }
}
