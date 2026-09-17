//! Stream header, stream footer and the check type they name.
//!
//! Spec §2.1.1, §2.1.2. C: `Xz_ParseHeader` and `Xz_ReadFooter` in `XzIn.c`.

use super::error::{XzErrorKind, XzResult};
use crate::crc::crc32;

/// `FD 37 7A 58 5A 00`, the six bytes every xz stream starts with.
pub const XZ_MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];
/// `YZ`, the two bytes every xz stream ends with.
pub const XZ_FOOTER_MAGIC: [u8; 2] = *b"YZ";
/// Size of a stream header, in bytes.
pub const STREAM_HEADER_SIZE: usize = 12;
/// Size of a stream footer, in bytes.
pub const STREAM_FOOTER_SIZE: usize = 12;

/// Which integrity check a stream's blocks carry.
///
/// Spec §2.1.1.2. The four the format defines are here; the twelve reserved
/// ids have a known size but no algorithm, so they are carried as
/// [`CheckType::Reserved`] and can only be skipped, never verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckType {
    /// No check at all (id 0).
    None,
    /// CRC-32/ISO-HDLC (id 1), four bytes.
    Crc32,
    /// CRC-64/XZ (id 4), eight bytes.
    Crc64,
    /// SHA-256 (id 10), thirty-two bytes.
    Sha256,
    /// One of the reserved ids. Its size is known, so it can be skipped.
    Reserved(u8),
}

impl CheckType {
    /// The check for a stream-flags check id, which is a four-bit field.
    ///
    /// # Errors
    ///
    /// [`XzErrorKind::BadStreamFlags`] if `id > 0x0F`.
    pub fn from_id(id: u8) -> Result<Self, XzErrorKind> {
        Ok(match id {
            0x00 => CheckType::None,
            0x01 => CheckType::Crc32,
            0x04 => CheckType::Crc64,
            0x0A => CheckType::Sha256,
            0x02 | 0x03 | 0x05..=0x09 | 0x0B..=0x0F => CheckType::Reserved(id),
            _ => return Err(XzErrorKind::BadStreamFlags),
        })
    }

    /// The four-bit id this check is stored as.
    #[must_use]
    pub fn id(self) -> u8 {
        match self {
            CheckType::None => 0x00,
            CheckType::Crc32 => 0x01,
            CheckType::Crc64 => 0x04,
            CheckType::Sha256 => 0x0A,
            CheckType::Reserved(id) => id,
        }
    }

    /// How many bytes this check occupies after a block's padding.
    ///
    /// Spec §2.1.1.2: ids 1-3 are four bytes, 4-6 eight, 7-9 sixteen, 10-12
    /// thirty-two and 13-15 sixty-four, whether or not the id is defined.
    #[must_use]
    pub fn size(self) -> usize {
        let id = self.id();
        if id == 0 { 0 } else { 4usize << ((id - 1) / 3) }
    }

    /// Whether this build can actually compute this check.
    ///
    /// SHA-256 needs `crypto` or `native-crypto`; without either, a SHA-256
    /// stream can still be decoded, but only if the caller opts out of
    /// verification.
    #[must_use]
    pub fn is_verifiable(self) -> bool {
        match self {
            CheckType::None | CheckType::Crc32 | CheckType::Crc64 => true,
            #[cfg(any(feature = "crypto", feature = "native-crypto"))]
            CheckType::Sha256 => true,
            #[cfg(not(any(feature = "crypto", feature = "native-crypto")))]
            CheckType::Sha256 => false,
            CheckType::Reserved(_) => false,
        }
    }
}

/// The two stream-flag bytes, which say only which check the blocks carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamFlags {
    /// The check every block in the stream is followed by.
    pub check: CheckType,
    /// The flag bytes as stored, kept so the footer can be compared with the
    /// header byte for byte as the spec requires.
    pub raw: [u8; 2],
}

impl StreamFlags {
    /// Parses the two flag bytes.
    ///
    /// # Errors
    ///
    /// [`XzErrorKind::BadStreamFlags`] if the first byte is not zero or any
    /// reserved bit of the second is set: a decoder that does not understand
    /// a flag cannot know it parsed the rest correctly.
    pub fn parse(raw: [u8; 2]) -> Result<Self, XzErrorKind> {
        if raw[0] != 0 || raw[1] & 0xF0 != 0 {
            return Err(XzErrorKind::BadStreamFlags);
        }
        Ok(StreamFlags {
            check: CheckType::from_id(raw[1] & 0x0F)?,
            raw,
        })
    }
}

/// A parsed stream header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamHeader {
    /// The stream's flags.
    pub flags: StreamFlags,
}

impl StreamHeader {
    /// Parses the twelve bytes of a stream header.
    ///
    /// # Errors
    ///
    /// [`XzErrorKind::BadMagic`] if the magic is wrong, [`XzErrorKind::HeaderCrc`]
    /// if the flags' CRC-32 does not match, and whatever [`StreamFlags::parse`]
    /// returns for the flags themselves. The CRC is checked *before* the flags
    /// are trusted, which is how a corrupt stream is told from an unsupported
    /// one (spec §2.1.1.3).
    pub fn parse(buf: &[u8; STREAM_HEADER_SIZE]) -> Result<Self, XzErrorKind> {
        if buf[..6] != XZ_MAGIC {
            return Err(XzErrorKind::BadMagic);
        }
        let stored = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        if crc32(&buf[6..8]) != stored {
            return Err(XzErrorKind::HeaderCrc);
        }
        Ok(StreamHeader {
            flags: StreamFlags::parse([buf[6], buf[7]])?,
        })
    }
}

/// A parsed stream footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamFooter {
    /// The stream's flags, which must equal the header's.
    pub flags: StreamFlags,
    /// The size of the index, in bytes, as the footer records it.
    pub index_size: u64,
}

impl StreamFooter {
    /// Parses the twelve bytes of a stream footer.
    ///
    /// # Errors
    ///
    /// [`XzErrorKind::BadFooterMagic`], [`XzErrorKind::HeaderCrc`], or a flags
    /// error. As in the header, the CRC is checked first.
    pub fn parse(buf: &[u8; STREAM_FOOTER_SIZE]) -> Result<Self, XzErrorKind> {
        if buf[10..12] != XZ_FOOTER_MAGIC {
            return Err(XzErrorKind::BadFooterMagic);
        }
        let stored = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        if crc32(&buf[4..10]) != stored {
            return Err(XzErrorKind::HeaderCrc);
        }
        let backward = u64::from(u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]));
        Ok(StreamFooter {
            flags: StreamFlags::parse([buf[8], buf[9]])?,
            // Spec §2.1.2.2: real_backward_size = (stored + 1) * 4. Both
            // operands are bounded by u32, so this cannot overflow u64.
            index_size: (backward + 1) * 4,
        })
    }
}

/// Whether `data` begins with an xz stream header this crate can read, and
/// with what check.
///
/// Cheap: it looks at twelve bytes and decodes nothing. Returns `None` for
/// anything that is not a well-formed header, including a buffer too short to
/// hold one.
#[must_use]
pub fn probe(data: &[u8]) -> Option<StreamHeader> {
    let buf: &[u8; STREAM_HEADER_SIZE] = data.get(..STREAM_HEADER_SIZE)?.try_into().ok()?;
    StreamHeader::parse(buf).ok()
}

/// Reads the twelve-byte footer that is `at` bytes into `data`.
#[allow(dead_code)] // used by the seekable paths added in the next commit
pub(crate) fn footer_at(data: &[u8], at: usize) -> XzResult<StreamFooter> {
    let buf: &[u8; STREAM_FOOTER_SIZE] = data
        .get(at..at + STREAM_FOOTER_SIZE)
        .and_then(|s| s.try_into().ok())
        .ok_or(super::error::XzError::at(
            XzErrorKind::TruncatedInput,
            0,
            at as u64,
        ))?;
    StreamFooter::parse(buf).map_err(|k| super::error::XzError::at(k, 0, at as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_bytes(check: u8) -> [u8; STREAM_HEADER_SIZE] {
        let mut buf = [0u8; STREAM_HEADER_SIZE];
        buf[..6].copy_from_slice(&XZ_MAGIC);
        buf[7] = check;
        let c = crc32(&buf[6..8]).to_le_bytes();
        buf[8..12].copy_from_slice(&c);
        buf
    }

    #[test]
    fn check_sizes_follow_the_spec_table() {
        assert_eq!(CheckType::None.size(), 0);
        assert_eq!(CheckType::Crc32.size(), 4);
        assert_eq!(CheckType::Reserved(3).size(), 4);
        assert_eq!(CheckType::Crc64.size(), 8);
        assert_eq!(CheckType::Reserved(6).size(), 8);
        assert_eq!(CheckType::Reserved(7).size(), 16);
        assert_eq!(CheckType::Sha256.size(), 32);
        assert_eq!(CheckType::Reserved(0x0F).size(), 64);
    }

    #[test]
    fn parses_a_header_and_rejects_a_flipped_bit() {
        let buf = header_bytes(0x04);
        let h = StreamHeader::parse(&buf).expect("header");
        assert_eq!(h.flags.check, CheckType::Crc64);

        let mut bad = buf;
        bad[7] ^= 0x01;
        assert_eq!(StreamHeader::parse(&bad), Err(XzErrorKind::HeaderCrc));

        let mut bad = buf;
        bad[0] = 0;
        assert_eq!(StreamHeader::parse(&bad), Err(XzErrorKind::BadMagic));
    }

    #[test]
    fn a_reserved_flag_bit_is_an_error_even_with_a_good_crc() {
        let mut buf = [0u8; STREAM_HEADER_SIZE];
        buf[..6].copy_from_slice(&XZ_MAGIC);
        buf[7] = 0x10;
        let c = crc32(&buf[6..8]).to_le_bytes();
        buf[8..12].copy_from_slice(&c);
        assert_eq!(StreamHeader::parse(&buf), Err(XzErrorKind::BadStreamFlags));
    }

    #[test]
    fn probe_says_no_to_short_and_wrong_buffers() {
        assert!(probe(&[]).is_none());
        assert!(probe(&XZ_MAGIC).is_none());
        assert!(probe(&header_bytes(0x01)).is_some());
    }
}
