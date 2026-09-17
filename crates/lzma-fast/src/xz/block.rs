//! Block headers.
//!
//! Spec §3.1. C: `XzBlock_Parse` in `XzDec.c`.
//!
//! The header's first byte is its own size, which makes it possible to read
//! the whole header before trusting anything in it: the CRC-32 at its end is
//! checked first, and only then are the flags, sizes and filters parsed. That
//! order is what tells a corrupt file from an unsupported one (spec §3.1.7),
//! and it is also what keeps a hostile size field from being used to allocate
//! anything.

use alloc::vec::Vec;

use super::error::XzErrorKind;
use super::filter::{FilterChain, FilterFlags, MAX_FILTERS};
use super::vli;
use crate::crc::crc32;

/// The largest a block header may be. Spec §3.1.1: the size byte encodes
/// `(value + 1) * 4`, so 0xFF gives 1024.
pub const MAX_BLOCK_HEADER_SIZE: usize = 1024;
/// The smallest a block header may be.
pub const MIN_BLOCK_HEADER_SIZE: usize = 8;

/// A parsed block header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockHeader {
    /// The header's size in bytes, including the size byte and the CRC-32.
    pub header_size: usize,
    /// The compressed size the header declares, if it declares one. This is
    /// the size of the compressed data alone, without padding or check.
    pub compressed_size: Option<u64>,
    /// The uncompressed size the header declares, if it declares one.
    pub uncompressed_size: Option<u64>,
    /// The filter chain, validated and in decode order.
    pub chain: FilterChain,
}

/// The real size of a block header from its first byte, or `None` if that
/// byte is the index indicator.
#[must_use]
pub fn header_size_from_first_byte(b: u8) -> Option<usize> {
    if b == 0 {
        None
    } else {
        Some((usize::from(b) + 1) * 4)
    }
}

impl BlockHeader {
    /// Parses a complete block header, which `buf` must hold exactly.
    ///
    /// # Errors
    ///
    /// [`XzErrorKind::BadBlockHeader`] for a size byte or flags out of range,
    /// [`XzErrorKind::HeaderCrc`] for a CRC mismatch, [`XzErrorKind::BadPadding`]
    /// for non-null padding, and whatever the filter and VLI parsers return.
    pub fn parse(buf: &[u8]) -> Result<Self, XzErrorKind> {
        if buf.len() < MIN_BLOCK_HEADER_SIZE
            || buf.len() > MAX_BLOCK_HEADER_SIZE
            || !buf.len().is_multiple_of(4)
        {
            return Err(XzErrorKind::BadBlockHeader);
        }
        if header_size_from_first_byte(buf[0]) != Some(buf.len()) {
            return Err(XzErrorKind::BadBlockHeader);
        }
        let body = &buf[..buf.len() - 4];
        let stored = u32::from_le_bytes([
            buf[buf.len() - 4],
            buf[buf.len() - 3],
            buf[buf.len() - 2],
            buf[buf.len() - 1],
        ]);
        // Checked before any field below is believed. Spec §3.1.7.
        if crc32(body) != stored {
            return Err(XzErrorKind::HeaderCrc);
        }

        let flags = body[1];
        if flags & 0x3C != 0 {
            // Reserved bits: there may be a field here we cannot see.
            return Err(XzErrorKind::BadBlockHeader);
        }
        let num_filters = usize::from(flags & 0x03) + 1;
        debug_assert!(num_filters <= MAX_FILTERS);

        let mut pos = 2usize;
        let compressed_size = if flags & 0x40 != 0 {
            let v = vli::decode_at(body, &mut pos)?;
            // Spec §3.1.3: the compressed size MUST be non-zero.
            if v == 0 {
                return Err(XzErrorKind::BadBlockHeader);
            }
            Some(v)
        } else {
            None
        };
        let uncompressed_size = if flags & 0x80 != 0 {
            Some(vli::decode_at(body, &mut pos)?)
        } else {
            None
        };

        let mut filters: Vec<FilterFlags> = Vec::with_capacity(num_filters);
        for _ in 0..num_filters {
            filters.push(FilterFlags::parse(body, &mut pos)?);
        }

        // Spec §3.1.6: the rest of the header is null padding.
        let tail = body.get(pos..).ok_or(XzErrorKind::BadBlockHeader)?;
        if tail.iter().any(|&b| b != 0) {
            return Err(XzErrorKind::BadPadding);
        }

        Ok(BlockHeader {
            header_size: buf.len(),
            compressed_size,
            uncompressed_size,
            chain: FilterChain::validate(&filters)?,
        })
    }

    /// Unpadded size of the block if its compressed size holds: the header,
    /// the compressed data and the check, with no block padding.
    ///
    /// Returns `None` when the header does not declare a compressed size.
    #[must_use]
    pub fn unpadded_size(&self, check_size: usize) -> Option<u64> {
        let c = self.compressed_size?;
        (self.header_size as u64)
            .checked_add(c)?
            .checked_add(check_size as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a header the way `xz` does, so the parser is tested against
    /// bytes rather than against itself.
    fn build(flags: u8, fields: &[u8]) -> Vec<u8> {
        let mut body = alloc::vec![0u8, flags];
        body.extend_from_slice(fields);
        // Pad to a multiple of four with room for the CRC.
        while (body.len() + 4) % 4 != 0 {
            body.push(0);
        }
        let size = body.len() + 4;
        body[0] = (size / 4 - 1) as u8;
        let c = crc32(&body).to_le_bytes();
        body.extend_from_slice(&c);
        body
    }

    #[test]
    fn parses_a_plain_lzma2_header() {
        let buf = build(0x00, &[0x21, 0x01, 0x18]);
        let h = BlockHeader::parse(&buf).expect("header");
        assert_eq!(h.chain.dict_prop, 0x18);
        assert!(h.chain.is_plain_lzma2());
        assert_eq!(h.compressed_size, None);
        assert_eq!(h.uncompressed_size, None);
    }

    #[test]
    fn parses_sizes_and_a_bcj_chain() {
        // flags: two filters, both sizes present.
        let buf = build(
            0xC1,
            &[0x80, 0x02, 0x80, 0x04, 0x04, 0x00, 0x21, 0x01, 0x18],
        );
        let h = BlockHeader::parse(&buf).expect("header");
        assert_eq!(h.compressed_size, Some(256));
        assert_eq!(h.uncompressed_size, Some(512));
        assert_eq!(h.chain.converters.len(), 1);
        assert_eq!(h.chain.converters[0].id, 0x04);
    }

    #[test]
    fn refuses_a_bad_crc_a_reserved_flag_and_dirty_padding() {
        let mut buf = build(0x00, &[0x21, 0x01, 0x18]);
        let n = buf.len();
        buf[n - 1] ^= 0x01;
        assert_eq!(BlockHeader::parse(&buf), Err(XzErrorKind::HeaderCrc));

        let buf = build(0x04, &[0x21, 0x01, 0x18]);
        assert_eq!(BlockHeader::parse(&buf), Err(XzErrorKind::BadBlockHeader));

        // A header whose padding is not null: build one by hand.
        let mut body = alloc::vec![0u8, 0x00, 0x21, 0x01, 0x18, 0x00, 0x00, 0x01];
        body[0] = ((body.len() + 4) / 4 - 1) as u8;
        let c = crc32(&body).to_le_bytes();
        body.extend_from_slice(&c);
        assert_eq!(BlockHeader::parse(&body), Err(XzErrorKind::BadPadding));
    }

    #[test]
    fn a_zero_compressed_size_is_refused() {
        let buf = build(0x40, &[0x00, 0x21, 0x01, 0x18]);
        assert_eq!(BlockHeader::parse(&buf), Err(XzErrorKind::BadBlockHeader));
    }
}
