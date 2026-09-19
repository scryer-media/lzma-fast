//! The `.xz` container writer.
//!
//! There is no `.xz` encoder in the LZMA SDK — 7-Zip writes `.xz` from
//! `C/XzEnc.c`, which is a thin frame around `Lzma2Enc.c`, and this is the
//! same frame written against the format specification (`xz-file-format.txt`
//! version 1.2.1) and against what [`crate::xz`] already parses. Every field
//! below names the spec section it comes from; the compressed data itself is
//! the port in [`super::lzma2_enc`].
//!
//! What it writes is one stream: a header, one or more blocks each declaring
//! both of its sizes, an index over them, and a footer. The filter chain is a
//! bare LZMA2 filter unless the caller asks for more with
//! [`XzEncoder::set_filters`], in which case the delta and BCJ converters run
//! over each block before LZMA2 sees it and the block header lists them in
//! that order. Which filter suits which file is a policy question the format
//! does not answer and `xz` only answers with command-line flags, so this
//! writer does not choose for the caller either.

use alloc::vec::Vec;

use crate::error::Error;
use crate::xz::filter::{FILTER_LZMA2, FilterChain, FilterFlags, MAX_FILTERS};
use crate::xz::stream::{CheckType, XZ_FOOTER_MAGIC, XZ_MAGIC};
use crate::xz::vli;

use super::lzma2_enc::Lzma2Encoder;
use super::props::LzmaEncProps;

/// The default block size: what one block may decode to before the writer
/// starts another.
///
/// `xz` sizes its blocks from the dictionary (`--block-size` otherwise), and
/// only when it is compressing in parallel; single-threaded it writes one
/// block for the whole file. This writer defaults to the same single block,
/// because a block boundary costs compression ratio — the dictionary resets
/// across it — and buys only parallel decoding, which is the caller's call.
pub const DEFAULT_BLOCK_SIZE: u64 = u64::MAX;

/// The largest block header the format allows. Spec §3.1.1.
const MAX_BLOCK_HEADER_SIZE: usize = 1024;

/// How many bytes a check of this type occupies.
fn check_size(check: CheckType) -> usize {
    check.size()
}

/// Computes one block's check over its uncompressed bytes.
///
/// The check types are exactly the ones this build can compute: spec §2.1.1.2
/// allows a decoder to skip a check it does not know, but a writer may not
/// write one it cannot produce, so an unavailable type is a parameter error
/// rather than a silently weaker stream.
fn compute_check(check: CheckType, data: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
    match check {
        CheckType::None => {}
        CheckType::Crc32 => out.extend_from_slice(&crate::crc::crc32(data).to_le_bytes()),
        CheckType::Crc64 => out.extend_from_slice(&crate::crc::crc64_xz(data).to_le_bytes()),
        #[cfg(any(feature = "crypto", feature = "native-crypto"))]
        CheckType::Sha256 => {
            let mut h = crate::crypto::Sha256::new();
            h.update(data);
            out.extend_from_slice(&h.finalize());
        }
        #[cfg(not(any(feature = "crypto", feature = "native-crypto")))]
        CheckType::Sha256 => return Err(Error::Param),
        CheckType::Reserved(_) => return Err(Error::Param),
    }
    Ok(())
}

/// Whether this build can compute `check` at all.
fn check_supported(check: CheckType) -> bool {
    match check {
        CheckType::None | CheckType::Crc32 | CheckType::Crc64 => true,
        CheckType::Sha256 => cfg!(any(feature = "crypto", feature = "native-crypto")),
        CheckType::Reserved(_) => false,
    }
}

/// An `.xz` stream writer.
///
/// Push bytes with [`XzEncoder::push`], end the stream with
/// [`XzEncoder::finish`], and take what has been produced so far with
/// [`XzEncoder::take_output`]. The output is produced a block at a time, so a
/// caller that drains after every push never holds more than one block's
/// compressed bytes plus the index.
pub struct XzEncoder {
    check: CheckType,
    block_size: u64,
    lzma2: Lzma2Encoder,
    /// The non-last filters, in the order they are applied and listed, empty
    /// for a bare LZMA2 chain.
    filters: Vec<FilterFlags>,
    /// Bytes waiting to become a block.
    pending: Vec<u8>,
    /// One `(unpadded size, uncompressed size)` per block written. Spec §4.2.
    records: Vec<(u64, u64)>,
    out: Vec<u8>,
    finished: bool,
}

impl XzEncoder {
    /// A writer for a single stream with the given LZMA2 settings.
    ///
    /// # Errors
    ///
    /// [`Error::Param`] if a setting is out of range or `lc + lp` is above 4,
    /// which LZMA2 does not allow, and [`Error::Alloc`] if the encoder state
    /// could not be allocated.
    pub fn new(props: &LzmaEncProps) -> Result<Self, Error> {
        let lzma2 = Lzma2Encoder::new(props)?;
        let mut enc = XzEncoder {
            check: CheckType::Crc64,
            block_size: DEFAULT_BLOCK_SIZE,
            lzma2,
            filters: Vec::new(),
            pending: Vec::new(),
            records: Vec::new(),
            out: Vec::new(),
            finished: false,
        };
        enc.write_stream_header();
        Ok(enc)
    }

    /// The check to put after every block. Spec §2.1.1.2; the default is
    /// CRC-64, which is what `xz` writes.
    ///
    /// # Errors
    ///
    /// [`Error::Param`] for a check this build cannot compute — SHA-256
    /// without either crypto feature, or a reserved type.
    pub fn set_check(&mut self, check: CheckType) -> Result<(), Error> {
        if !check_supported(check) {
            return Err(Error::Param);
        }
        if !self.records.is_empty() || !self.pending.is_empty() {
            // The check type lives in the stream header, which is already
            // written and is repeated in the footer; changing it after bytes
            // have gone in would contradict it.
            return Err(Error::Param);
        }
        self.check = check;
        self.out.clear();
        self.write_stream_header();
        Ok(())
    }

    /// Sets the non-last filters of every block's chain, in the order they
    /// are applied: `[delta]`, `[bcj]`, `[delta, bcj]` and so on, with the
    /// LZMA2 filter appended for you.
    ///
    /// Each filter's state is reset at every block boundary, as the format
    /// requires, so a filtered stream may still be split into blocks.
    ///
    /// # Errors
    ///
    /// [`Error::Param`] if the chain is one [`FilterChain::validate`] refuses
    /// — more than three non-last filters, an LZMA2 filter among them, a
    /// filter this crate does not implement, or a misaligned BCJ start offset
    /// — or if bytes have already gone in.
    pub fn set_filters(&mut self, filters: &[FilterFlags]) -> Result<(), Error> {
        if !self.records.is_empty() || !self.pending.is_empty() {
            return Err(Error::Param);
        }
        if filters.len() >= MAX_FILTERS {
            return Err(Error::Param);
        }
        // Validate the chain a reader will see, which is these plus LZMA2.
        let _ = self.chain_for(filters)?;
        self.filters
            .try_reserve(filters.len())
            .map_err(|_| Error::Alloc)?;
        self.filters.clear();
        self.filters.extend_from_slice(filters);
        Ok(())
    }

    /// The non-last filters set with [`XzEncoder::set_filters`].
    #[must_use]
    pub fn filters(&self) -> &[FilterFlags] {
        &self.filters
    }

    /// The whole chain — the given filters plus this encoder's LZMA2 filter —
    /// validated the way [`crate::xz`] validates one it has just parsed.
    fn chain_for(&self, filters: &[FilterFlags]) -> Result<FilterChain, Error> {
        let mut whole: Vec<FilterFlags> = Vec::new();
        whole
            .try_reserve(filters.len() + 1)
            .map_err(|_| Error::Alloc)?;
        whole.extend_from_slice(filters);
        let mut props = [0u8; 4];
        props[0] = self.lzma2.properties();
        whole.push(FilterFlags {
            id: FILTER_LZMA2,
            props,
            props_len: 1,
        });
        FilterChain::validate(&whole).map_err(|_| Error::Param)
    }

    /// How much a single block may decode to before the writer starts
    /// another. Zero means the default (one block for everything).
    pub fn set_block_size(&mut self, bytes: u64) {
        self.block_size = if bytes == 0 {
            DEFAULT_BLOCK_SIZE
        } else {
            bytes
        };
    }

    /// The bytes produced so far, which the caller now owns.
    #[must_use]
    pub fn take_output(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.out)
    }

    /// The bytes produced so far, without taking them.
    #[must_use]
    pub fn output(&self) -> &[u8] {
        &self.out
    }

    /// Adds the next uncompressed bytes, emitting blocks as they fill.
    ///
    /// # Errors
    ///
    /// Whatever the LZMA2 encoder returns, or [`Error::Alloc`].
    pub fn push(&mut self, mut data: &[u8]) -> Result<(), Error> {
        if self.finished {
            return Err(Error::Param);
        }
        while !data.is_empty() {
            let room = self.block_size - self.pending.len() as u64;
            let take = core::cmp::min(room, data.len() as u64) as usize;
            self.pending.try_reserve(take).map_err(|_| Error::Alloc)?;
            self.pending.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.pending.len() as u64 >= self.block_size {
                self.emit_block()?;
            }
        }
        Ok(())
    }

    /// Ends the stream: flushes the last block, then writes the index and the
    /// footer.
    ///
    /// # Errors
    ///
    /// As [`XzEncoder::push`], plus [`Error::Param`] if the stream has already
    /// been finished.
    pub fn finish(&mut self) -> Result<(), Error> {
        if self.finished {
            return Err(Error::Param);
        }
        if !self.pending.is_empty() {
            self.emit_block()?;
        }
        self.write_index();
        self.write_footer();
        self.finished = true;
        Ok(())
    }

    /// Spec §2.1.1: the magic, the stream flags, and a CRC-32 over the flags.
    fn write_stream_header(&mut self) {
        self.out.extend_from_slice(&XZ_MAGIC);
        let flags = self.stream_flags();
        self.out.extend_from_slice(&flags);
        self.out
            .extend_from_slice(&crate::crc::crc32(&flags).to_le_bytes());
    }

    /// Spec §2.1.1.2: a null byte, then the check id in the low four bits.
    fn stream_flags(&self) -> [u8; 2] {
        [0, self.check.id()]
    }

    /// Compresses [`XzEncoder::pending`] and appends one whole block.
    ///
    /// Spec §3: header, compressed data, padding to a multiple of four, then
    /// the check.
    fn emit_block(&mut self) -> Result<(), Error> {
        let mut data = core::mem::take(&mut self.pending);
        let uncompressed = data.len() as u64;

        // §3.4: the check covers the block's *uncompressed* data, which is
        // what came in, not what the filters made of it — so take it before
        // they run. They are size-preserving, so the header's uncompressed
        // size is the same either way.
        let mut check_bytes = Vec::new();
        compute_check(self.check, &data, &mut check_bytes)?;

        if !self.filters.is_empty() {
            // Fresh converters for every block: the format resets filter
            // state at each block boundary, and the reader builds them the
            // same way.
            let chain = self.chain_for(&self.filters)?;
            let mut convs = chain.build().map_err(|_| Error::Param)?;
            convs.encode_in_place(&mut data);
        }

        self.lzma2.set_data_size(uncompressed);
        let compressed = self.lzma2.encode_to_vec(&data)?;
        let dict_prop = self.lzma2.properties();

        let header = block_header(
            &self.filters,
            dict_prop,
            compressed.len() as u64,
            uncompressed,
        )?;
        let unpadded = header.len() + compressed.len() + check_size(self.check);

        self.out
            .try_reserve(unpadded + 3)
            .map_err(|_| Error::Alloc)?;
        self.out.extend_from_slice(&header);
        self.out.extend_from_slice(&compressed);
        // §3.2 Block Padding: null bytes up to a multiple of four.
        pad_to_four(&mut self.out, compressed.len());
        self.out.extend_from_slice(&check_bytes);

        self.records.try_reserve(1).map_err(|_| Error::Alloc)?;
        self.records.push((unpadded as u64, uncompressed));
        self.pending = data;
        self.pending.clear();
        Ok(())
    }

    /// Spec §4: the index, and the size it will make the footer declare.
    fn write_index(&mut self) {
        let start = self.out.len();
        self.out.push(0x00); // §4.1 Index Indicator.
        vli::push(self.records.len() as u64, &mut self.out);
        for &(unpadded, uncompressed) in &self.records {
            vli::push(unpadded, &mut self.out);
            vli::push(uncompressed, &mut self.out);
        }
        let body = self.out.len() - start;
        pad_to_four(&mut self.out, body);
        let crc = crate::crc::crc32(&self.out[start..]);
        self.out.extend_from_slice(&crc.to_le_bytes());
    }

    /// Spec §2.1.2: a CRC-32 over the two fields that follow it, the backward
    /// size, the stream flags, and the footer magic.
    fn write_footer(&mut self) {
        let index_size = self.index_size();
        let mut fields = [0u8; 6];
        // §2.1.2.1: the stored value is the real size in four-byte units,
        // less one.
        let backward = (index_size / 4 - 1) as u32;
        fields[..4].copy_from_slice(&backward.to_le_bytes());
        fields[4..].copy_from_slice(&self.stream_flags());
        self.out
            .extend_from_slice(&crate::crc::crc32(&fields).to_le_bytes());
        self.out.extend_from_slice(&fields);
        self.out.extend_from_slice(&XZ_FOOTER_MAGIC);
    }

    /// The index's size in bytes, padding and CRC included.
    fn index_size(&self) -> u64 {
        let mut n = 1 + vli::encoded_len(self.records.len() as u64);
        for &(unpadded, uncompressed) in &self.records {
            n += vli::encoded_len(unpadded) + vli::encoded_len(uncompressed);
        }
        (n as u64).next_multiple_of(4) + 4
    }
}

/// Appends null bytes until `written` bytes are a multiple of four.
fn pad_to_four(out: &mut Vec<u8>, written: usize) {
    let pad = written.next_multiple_of(4) - written;
    out.extend(core::iter::repeat_n(0u8, pad));
}

/// Builds one block header. Spec §3.1.
///
/// Both sizes are declared, which a decoder is required to check the block
/// against, and the filter chain is the single LZMA2 filter with its one
/// dictionary property byte (§5.3.1).
fn block_header(
    filters: &[FilterFlags],
    dict_prop: u8,
    compressed: u64,
    uncompressed: u64,
) -> Result<Vec<u8>, Error> {
    // §3.1.2 Block Flags: filter count minus one in bits 0-1, and the two
    // size-present bits.
    let flags = 0x40 | 0x80 | filters.len() as u8;

    let mut body = 2
        + vli::encoded_len(compressed)
        + vli::encoded_len(uncompressed)
        + 3 // the LZMA2 filter: id, property size, the property byte
        ;
    for f in filters {
        body += vli::encoded_len(f.id) + vli::encoded_len(f.props_len as u64) + f.props_len;
    }
    let size = body.next_multiple_of(4) + 4;
    if size > MAX_BLOCK_HEADER_SIZE {
        return Err(Error::Param);
    }

    let mut h = Vec::new();
    h.try_reserve(size).map_err(|_| Error::Alloc)?;
    // §3.1.1: the stored size is the real size in four-byte units, less one.
    h.push((size / 4 - 1) as u8);
    h.push(flags);
    vli::push(compressed, &mut h);
    vli::push(uncompressed, &mut h);
    // §3.1.5: the filters in the order the encoder applied them, LZMA2 last.
    for f in filters {
        vli::push(f.id, &mut h);
        vli::push(f.props_len as u64, &mut h);
        h.extend_from_slice(f.props());
    }
    vli::push(FILTER_LZMA2, &mut h);
    vli::push(1, &mut h); // §3.1.4: the size of the properties that follow.
    h.push(dict_prop);
    // §3.1.6 Header Padding, then §3.1.7 the CRC-32 over everything before it.
    h.resize(size - 4, 0);
    let crc = crate::crc::crc32(&h);
    h.extend_from_slice(&crc.to_le_bytes());
    debug_assert_eq!(h.len(), size);
    Ok(h)
}

/// Encode `src` as a whole `.xz` stream.
///
/// `check` is the per-block check, `block_size` the most one block may decode
/// to (zero for one block over the whole input).
///
/// # Errors
///
/// [`Error::Param`] if a setting is out of range or the check is one this
/// build cannot compute, [`Error::Alloc`] on allocation failure.
pub fn encode_xz(
    src: &[u8],
    props: &LzmaEncProps,
    check: CheckType,
    block_size: u64,
) -> Result<Vec<u8>, Error> {
    encode_xz_with_filters(src, props, check, block_size, &[])
}

/// Encode `src` as a whole `.xz` stream through a filter chain.
///
/// `filters` are the non-last filters in the order they are applied; the
/// LZMA2 filter is appended for you, so an empty slice is [`encode_xz`].
///
/// # Errors
///
/// As [`encode_xz`], plus [`Error::Param`] for a chain
/// [`XzEncoder::set_filters`] refuses.
pub fn encode_xz_with_filters(
    src: &[u8],
    props: &LzmaEncProps,
    check: CheckType,
    block_size: u64,
    filters: &[FilterFlags],
) -> Result<Vec<u8>, Error> {
    let mut enc = XzEncoder::new(props)?;
    enc.set_check(check)?;
    enc.set_filters(filters)?;
    enc.set_block_size(block_size);
    enc.push(src)?;
    enc.finish()?;
    Ok(enc.take_output())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_stream_is_header_empty_index_footer() {
        let props = LzmaEncProps::new();
        let out = encode_xz(b"", &props, CheckType::Crc64, 0).expect("encode");
        // 12 header + 8 index (indicator, count, two pad, CRC) + 12 footer.
        assert_eq!(out.len(), 32);
        assert_eq!(&out[..6], &XZ_MAGIC);
        assert_eq!(&out[out.len() - 2..], &XZ_FOOTER_MAGIC);
    }

    #[test]
    fn a_block_header_is_a_multiple_of_four_and_carries_its_crc() {
        let h = block_header(&[], 20, 1234, 65536).expect("header");
        assert!(h.len().is_multiple_of(4));
        assert_eq!(h[0] as usize, h.len() / 4 - 1);
        // §3.1.2: one filter, so the count bits are zero.
        assert_eq!(h[1], 0xC0);
        let crc = crate::crc::crc32(&h[..h.len() - 4]);
        assert_eq!(&h[h.len() - 4..], &crc.to_le_bytes());
    }

    #[test]
    fn a_filtered_block_header_lists_its_filters_in_order() {
        let delta = FilterFlags::new(crate::xz::filter::FILTER_DELTA, &[3]).expect("props");
        let bcj = FilterFlags::new(crate::xz::bcj::BcjKind::X86.filter_id(), &[]).expect("props");
        let h = block_header(&[delta, bcj], 20, 1234, 65536).expect("header");
        assert!(h.len().is_multiple_of(4));
        // §3.1.2: three filters in the chain, so the count bits hold two.
        assert_eq!(h[1], 0xC2);
        // The header parser must read back what was written, in the order it
        // was written: delta, then BCJ, then LZMA2.
        let parsed = crate::xz::BlockHeader::parse(&h).expect("parses");
        assert_eq!(parsed.chain.converters.len(), 2);
        // `converters` is the decode order, so it is the list reversed.
        assert_eq!(parsed.chain.converters[0].id, bcj.id);
        assert_eq!(parsed.chain.converters[1].id, delta.id);
        assert_eq!(parsed.chain.dict_prop, 20);
    }

    #[test]
    fn block_size_splits_the_input() {
        let props = LzmaEncProps::new().with_dict_size(1 << 16);
        let src: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
        let one = encode_xz(&src, &props, CheckType::Crc32, 0).expect("one block");
        let many = encode_xz(&src, &props, CheckType::Crc32, 16 * 1024).expect("five blocks");
        assert!(many.len() > one.len(), "more blocks cost ratio");
    }
}
