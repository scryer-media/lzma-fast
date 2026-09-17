//! The index, and the block table a seekable reader builds from it.
//!
//! Spec §4. C: `Xz_ReadIndex` / `Xz_ParseIndex` in `XzIn.c`.
//!
//! The index is what makes a multi-block stream decodable on several threads
//! without decoding anything first: every block's compressed and uncompressed
//! size is in it, so every block's file offset follows by addition. Reading it
//! costs one seek to the footer and one read of the index itself.

use alloc::vec::Vec;

use super::error::{XzError, XzErrorKind, XzResult};
use super::stream::{STREAM_FOOTER_SIZE, STREAM_HEADER_SIZE, StreamFooter, StreamHeader};
use super::vli;
use crate::crc::{Crc64Xz, crc32};

/// The largest index the spec allows: 16 GiB.
pub const MAX_INDEX_SIZE: u64 = 1 << 34;
/// The smallest a record can be: two one-byte VLIs.
const MIN_RECORD_BYTES: u64 = 2;

/// One index record: what the stream says a block costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XzIndexRecord {
    /// Block header + compressed data + check, without block padding.
    pub unpadded_size: u64,
    /// What the block decodes to.
    pub uncompressed_size: u64,
}

impl XzIndexRecord {
    /// The block's size with its padding, which is what the next block's
    /// offset is reached by adding.
    ///
    /// # Errors
    ///
    /// `None` on overflow, which a hostile index can ask for.
    #[must_use]
    pub fn padded_size(&self) -> Option<u64> {
        Some(self.unpadded_size.checked_add(3)? & !3)
    }
}

/// One block, located.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XzBlockEntry {
    /// Offset of the block's first header byte in the file.
    pub file_offset: u64,
    /// Offset of the block's first decoded byte in the stream's output.
    pub uncompressed_offset: u64,
    /// The record this came from.
    pub record: XzIndexRecord,
}

/// The blocks that went by, folded to a fixed size.
///
/// The index repeats, for every block, sizes the blocks themselves already
/// carried, and a decoder is required to check that the two agree (spec §4.2).
/// Holding the list would let a stream of millions of tiny blocks cost memory
/// proportional to the file; instead each record is folded into a CRC-64 as it
/// is produced, and the index's records are folded the same way as they are
/// read. Any disagreement shows up in the fold.
#[derive(Debug, Default)]
pub(crate) struct IndexFold {
    pub(crate) count: u64,
    blocks_size: u64,
    uncompressed: u64,
    digest: Crc64Xz,
}

impl IndexFold {
    pub(crate) fn push(&mut self, unpadded: u64, uncompressed: u64) -> Result<(), XzErrorKind> {
        self.count += 1;
        let padded = unpadded.checked_add(3).ok_or(XzErrorKind::SizeMismatch)? & !3;
        self.blocks_size = self
            .blocks_size
            .checked_add(padded)
            .ok_or(XzErrorKind::SizeMismatch)?;
        self.uncompressed = self
            .uncompressed
            .checked_add(uncompressed)
            .ok_or(XzErrorKind::SizeMismatch)?;
        self.digest.update(&unpadded.to_le_bytes());
        self.digest.update(&uncompressed.to_le_bytes());
        Ok(())
    }

    /// Closes the fold: count, the size of the blocks with their padding, the
    /// uncompressed size, and the digest of the records.
    pub(crate) fn finish(&mut self) -> (u64, u64, u64, u64) {
        let digest = core::mem::take(&mut self.digest).finalize();
        (self.count, self.blocks_size, self.uncompressed, digest)
    }
}

/// A parsed index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XzIndex {
    /// One record per block, in stream order.
    pub records: Vec<XzIndexRecord>,
    /// The index's own size in bytes, padding and CRC included.
    pub index_size: u64,
}

impl XzIndex {
    /// Parses an index from the bytes between the last block and the footer.
    ///
    /// `buf` must be exactly the index, including its padding and CRC-32.
    ///
    /// # Errors
    ///
    /// [`XzErrorKind::IndexMismatch`] if the indicator, record count or
    /// padding are wrong, [`XzErrorKind::HeaderCrc`] on a CRC mismatch, and
    /// the VLI errors for malformed integers. The record count is bounded by
    /// the bytes actually present before a single record is allocated.
    pub fn parse(buf: &[u8]) -> Result<Self, XzErrorKind> {
        if buf.len() < 8 || !buf.len().is_multiple_of(4) || buf.len() as u64 > MAX_INDEX_SIZE {
            return Err(XzErrorKind::IndexMismatch);
        }
        let body = &buf[..buf.len() - 4];
        let stored = u32::from_le_bytes([
            buf[buf.len() - 4],
            buf[buf.len() - 3],
            buf[buf.len() - 2],
            buf[buf.len() - 1],
        ]);
        if crc32(body) != stored {
            return Err(XzErrorKind::HeaderCrc);
        }
        if body[0] != 0x00 {
            return Err(XzErrorKind::IndexMismatch);
        }

        let mut pos = 1usize;
        let count = vli::decode_at(body, &mut pos)?;
        // Every record is at least two bytes, so a count larger than the
        // bytes left cannot be honest. This is checked before `with_capacity`
        // so that a nine-byte VLI cannot ask for a 2^63-entry allocation.
        let room = (body.len() - pos) as u64 / MIN_RECORD_BYTES;
        if count > room {
            return Err(XzErrorKind::IndexMismatch);
        }
        let mut records = Vec::new();
        records
            .try_reserve_exact(count as usize)
            .map_err(|_| XzErrorKind::IndexMismatch)?;

        let mut total_unpadded = 0u64;
        for _ in 0..count {
            let unpadded_size = vli::decode_at(body, &mut pos)?;
            let uncompressed_size = vli::decode_at(body, &mut pos)?;
            // Spec §4.3.1: never zero, and the real minimum is five.
            if unpadded_size < 5 {
                return Err(XzErrorKind::IndexMismatch);
            }
            let rec = XzIndexRecord {
                unpadded_size,
                uncompressed_size,
            };
            total_unpadded = total_unpadded
                .checked_add(rec.padded_size().ok_or(XzErrorKind::IndexMismatch)?)
                .ok_or(XzErrorKind::IndexMismatch)?;
            records.push(rec);
        }
        let _ = total_unpadded;

        // Spec §4.4: 0-3 null bytes of padding.
        let pad = body.get(pos..).ok_or(XzErrorKind::IndexMismatch)?;
        if pad.len() > 3 || pad.iter().any(|&b| b != 0) {
            return Err(XzErrorKind::IndexMismatch);
        }

        Ok(XzIndex {
            records,
            index_size: buf.len() as u64,
        })
    }

    /// How many blocks the stream has.
    #[must_use]
    pub fn block_count(&self) -> usize {
        self.records.len()
    }

    /// The stream's total uncompressed size.
    ///
    /// # Errors
    ///
    /// `None` if the records overflow, which only a malformed index can do.
    #[must_use]
    pub fn uncompressed_size(&self) -> Option<u64> {
        self.records
            .iter()
            .try_fold(0u64, |a, r| a.checked_add(r.uncompressed_size))
    }

    /// The sum of the blocks' padded sizes: the distance from the end of the
    /// stream header to the start of the index.
    ///
    /// # Errors
    ///
    /// `None` on overflow.
    #[must_use]
    pub fn blocks_size(&self) -> Option<u64> {
        self.records
            .iter()
            .try_fold(0u64, |a, r| a.checked_add(r.padded_size()?))
    }

    /// Where every block starts, given where the stream itself starts.
    ///
    /// # Errors
    ///
    /// `None` on overflow.
    #[must_use]
    pub fn blocks(&self, stream_offset: u64) -> Option<Vec<XzBlockEntry>> {
        let mut file_offset = stream_offset.checked_add(STREAM_HEADER_SIZE as u64)?;
        let mut uncompressed_offset = 0u64;
        let mut out = Vec::with_capacity(self.records.len());
        for &record in &self.records {
            out.push(XzBlockEntry {
                file_offset,
                uncompressed_offset,
                record,
            });
            file_offset = file_offset.checked_add(record.padded_size()?)?;
            uncompressed_offset = uncompressed_offset.checked_add(record.uncompressed_size)?;
        }
        Some(out)
    }

    /// The largest uncompressed block in the stream, which is what a parallel
    /// decode has to be able to hold per worker.
    #[must_use]
    pub fn largest_block(&self) -> u64 {
        self.records
            .iter()
            .map(|r| r.uncompressed_size)
            .max()
            .unwrap_or(0)
    }
}

/// A stream located from its footer: where it starts, its index, its flags.
#[derive(Debug, Clone)]
pub struct XzStreamIndex {
    /// Offset of the stream's first header byte in the file.
    pub stream_offset: u64,
    /// Total size of the stream, header to footer inclusive.
    pub stream_size: u64,
    /// The stream's header, whose flags the footer's must match.
    pub header: StreamHeader,
    /// The index.
    pub index: XzIndex,
}

/// Reads the index of the stream that *ends* at `end` in `reader`, by seeking
/// to its footer first.
///
/// This is the seekable path: nothing is decoded and only the footer, the
/// index and the stream header are read.
///
/// # Errors
///
/// Any structural error in the footer, index or header, and I/O errors from
/// `reader`, which are reported as [`XzErrorKind::TruncatedInput`] when they
/// are a short read.
pub fn read_stream_index_ending_at<R: std::io::Read + std::io::Seek>(
    reader: &mut R,
    end: u64,
    memory_limit: u64,
) -> XzResult<XzStreamIndex> {
    use std::io::SeekFrom;

    let at = |off: u64| XzError::at(XzErrorKind::TruncatedInput, 0, off);
    let footer_offset = end
        .checked_sub(STREAM_FOOTER_SIZE as u64)
        .ok_or_else(|| at(end))?;
    reader
        .seek(SeekFrom::Start(footer_offset))
        .map_err(|_| at(footer_offset))?;
    let mut footer = [0u8; STREAM_FOOTER_SIZE];
    reader
        .read_exact(&mut footer)
        .map_err(|_| at(footer_offset))?;
    let footer = StreamFooter::parse(&footer).map_err(|k| XzError::at(k, 0, footer_offset))?;

    if footer.index_size > MAX_INDEX_SIZE || footer.index_size > memory_limit {
        return Err(XzError::at(
            XzErrorKind::MemoryLimit {
                needed: footer.index_size,
                limit: memory_limit,
            },
            0,
            footer_offset,
        ));
    }
    let index_offset = footer_offset
        .checked_sub(footer.index_size)
        .ok_or_else(|| at(footer_offset))?;
    if index_offset < STREAM_HEADER_SIZE as u64 {
        return Err(XzError::at(XzErrorKind::IndexMismatch, 0, index_offset));
    }

    let mut index_buf = Vec::new();
    index_buf
        .try_reserve_exact(footer.index_size as usize)
        .map_err(|_| {
            XzError::at(
                XzErrorKind::MemoryLimit {
                    needed: footer.index_size,
                    limit: memory_limit,
                },
                0,
                index_offset,
            )
        })?;
    index_buf.resize(footer.index_size as usize, 0u8);
    reader
        .seek(SeekFrom::Start(index_offset))
        .map_err(|_| at(index_offset))?;
    reader
        .read_exact(&mut index_buf)
        .map_err(|_| at(index_offset))?;
    let index = XzIndex::parse(&index_buf).map_err(|k| XzError::at(k, 0, index_offset))?;

    let blocks_size = index
        .blocks_size()
        .ok_or_else(|| XzError::at(XzErrorKind::IndexMismatch, 0, index_offset))?;
    // The index must sit exactly where the blocks end, which is what ties the
    // record list to the file rather than to itself.
    let stream_offset = index_offset
        .checked_sub(blocks_size)
        .and_then(|v| v.checked_sub(STREAM_HEADER_SIZE as u64))
        .ok_or_else(|| XzError::at(XzErrorKind::IndexMismatch, 0, index_offset))?;

    reader
        .seek(SeekFrom::Start(stream_offset))
        .map_err(|_| at(stream_offset))?;
    let mut head = [0u8; STREAM_HEADER_SIZE];
    reader
        .read_exact(&mut head)
        .map_err(|_| at(stream_offset))?;
    let header = StreamHeader::parse(&head).map_err(|k| XzError::at(k, 0, stream_offset))?;
    if header.flags.raw != footer.flags.raw {
        return Err(XzError::at(
            XzErrorKind::StreamFlagsMismatch,
            0,
            footer_offset,
        ));
    }

    Ok(XzStreamIndex {
        stream_offset,
        stream_size: end - stream_offset,
        header,
        index,
    })
}

/// How many blocks the single xz stream in `reader` has, or `None` if the
/// file is not one structurally sound stream.
///
/// This is the gate a caller uses to decide between a sequential decode and a
/// parallel one *without decoding anything*: a file that is several
/// concatenated streams, that has stream padding, or whose index does not add
/// up, answers `None` and is left to the sequential reader, which is the thing
/// that validates it properly.
///
/// The caller's stream position is not restored; do that yourself if you care.
///
/// # Errors
///
/// None: every failure is `None`. This is a structural gate, not a validator.
pub fn single_stream_block_count<R: std::io::Read + std::io::Seek>(
    reader: &mut R,
) -> Option<usize> {
    use std::io::SeekFrom;

    let len = reader.seek(SeekFrom::End(0)).ok()?;
    let idx = read_stream_index_ending_at(reader, len, MAX_INDEX_SIZE).ok()?;
    // Exactly one stream, starting at zero, with nothing after it.
    (idx.stream_offset == 0).then_some(idx.index.block_count())
}

/// Whether `reader` holds one xz stream with more than one block, which is
/// the only shape a block-parallel decode helps with.
///
/// # Errors
///
/// None; see [`single_stream_block_count`].
pub fn is_single_stream_multi_block<R: std::io::Read + std::io::Seek>(reader: &mut R) -> bool {
    single_stream_block_count(reader).is_some_and(|n| n > 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_index(records: &[(u64, u64)]) -> Vec<u8> {
        let mut body = alloc::vec![0x00u8];
        let push_vli = |body: &mut Vec<u8>, mut v: u64| {
            while v >= 0x80 {
                body.push((v as u8) | 0x80);
                v >>= 7;
            }
            body.push(v as u8);
        };
        push_vli(&mut body, records.len() as u64);
        for &(u, c) in records {
            push_vli(&mut body, u);
            push_vli(&mut body, c);
        }
        while (body.len() + 4) % 4 != 0 {
            body.push(0);
        }
        let crc = crc32(&body).to_le_bytes();
        body.extend_from_slice(&crc);
        body
    }

    #[test]
    fn parses_records_and_locates_blocks() {
        let buf = build_index(&[(100, 1000), (57, 2000)]);
        let idx = XzIndex::parse(&buf).expect("index");
        assert_eq!(idx.block_count(), 2);
        assert_eq!(idx.uncompressed_size(), Some(3000));
        // 100 rounds to 100, 57 rounds to 60.
        assert_eq!(idx.blocks_size(), Some(160));
        let blocks = idx.blocks(0).expect("blocks");
        assert_eq!(blocks[0].file_offset, 12);
        assert_eq!(blocks[1].file_offset, 112);
        assert_eq!(blocks[1].uncompressed_offset, 1000);
    }

    #[test]
    fn refuses_a_record_count_larger_than_the_bytes_present() {
        let mut buf = build_index(&[(100, 1000)]);
        // Rewrite the count as 0x7F, which cannot fit in the bytes left.
        buf[1] = 0x7F;
        let crc = crc32(&buf[..buf.len() - 4]).to_le_bytes();
        let n = buf.len();
        buf[n - 4..].copy_from_slice(&crc);
        assert_eq!(XzIndex::parse(&buf), Err(XzErrorKind::IndexMismatch));
    }

    #[test]
    fn refuses_a_zero_unpadded_size_and_a_broken_crc() {
        let buf = build_index(&[(0, 10)]);
        assert_eq!(XzIndex::parse(&buf), Err(XzErrorKind::IndexMismatch));

        let mut buf = build_index(&[(100, 1000)]);
        let n = buf.len();
        buf[n - 1] ^= 0xFF;
        assert_eq!(XzIndex::parse(&buf), Err(XzErrorKind::HeaderCrc));
    }
}
