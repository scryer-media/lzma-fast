//! Verifying a block's check, in the decoder that produced the block.
//!
//! C: `XzCheck_Update` / `XzCheck_Final` in `XzDec.c`, which run on the
//! decoding thread. This does too - and it is the same machinery the LZMA2
//! workers already use ([`crate::checksum`]), so a block decoded by a worker
//! is checked by that worker and never on the thread draining the output.
//!
//! Because the caller may also want the decoded bytes cut at boundaries of its
//! own, the check is computed as *segments* and folded into the block's single
//! value with [`crate::crc::CrcFolder`]. One pass over the bytes either way:
//! the fold is arithmetic on the segment values, not a re-read.

use alloc::vec::Vec;

use super::error::XzErrorKind;
use super::stream::CheckType;
use crate::crc::CrcFolder;
use crate::mt::checksum::{BlockChecks, Checksum, ChecksumPlan, Segment, Segmenter};

/// The check of one block, as produced by whoever decoded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockCheck {
    /// Absolute offset of the block's first byte in the decoded stream.
    pub unpacked_offset: u64,
    /// The block's length in bytes.
    pub len: u64,
    /// The pieces the caller's split points cut the block into, with their
    /// CRCs. Empty for a stream with no check, for a SHA-256 stream, and for
    /// a decode that was not asked to verify.
    pub segments: Vec<Segment>,
    /// SHA-256 of the whole block, for a check type 10 stream.
    pub digest: Option<[u8; 32]>,
}

/// A check being computed over a block as it decodes.
pub(crate) struct RunningCheck {
    kind: CheckType,
    seg: Option<Segmenter>,
    start: u64,
}

impl RunningCheck {
    /// A check for a block starting at unpacked offset `start`.
    ///
    /// `splits` are the caller's absolute boundaries; they only ever refine
    /// the block's own single value, which is recovered by folding.
    pub(crate) fn new(kind: CheckType, start: u64, splits: &ChecksumPlan, verify: bool) -> Self {
        let plan_kind = match kind {
            CheckType::Crc32 => Checksum::Crc32,
            CheckType::Crc64 => Checksum::Crc64Xz,
            #[cfg(any(feature = "crypto", feature = "native-crypto"))]
            CheckType::Sha256 => Checksum::Sha256,
            #[cfg(not(any(feature = "crypto", feature = "native-crypto")))]
            CheckType::Sha256 => Checksum::None,
            CheckType::None | CheckType::Reserved(_) => Checksum::None,
        };
        let seg = if verify && plan_kind != Checksum::None {
            let plan = ChecksumPlan::new(plan_kind)
                .with_split_points(splits.split_points().iter().copied());
            Some(Segmenter::new(&plan, start))
        } else {
            None
        };
        RunningCheck { kind, seg, start }
    }

    /// Feeds the next decoded bytes of the block.
    pub(crate) fn update(&mut self, data: &[u8]) {
        if let Some(s) = self.seg.as_mut() {
            s.update(data);
        }
    }

    /// Closes the block and compares what was computed with the `stored`
    /// bytes of the check field.
    ///
    /// Returns the block's segments and digest for the caller.
    ///
    /// # Errors
    ///
    /// [`XzErrorKind::CheckMismatch`] if the values differ.
    pub(crate) fn finish(self, stored: &[u8]) -> Result<BlockCheck, XzErrorKind> {
        let Some(seg) = self.seg else {
            return Ok(BlockCheck {
                unpacked_offset: self.start,
                len: 0,
                segments: Vec::new(),
                digest: None,
            });
        };
        let BlockChecks {
            unpacked_offset,
            len,
            digest,
            segments,
        } = seg.finish();

        match self.kind {
            CheckType::Crc32 => {
                let mut f = CrcFolder::<u32>::new();
                for s in &segments {
                    if let Some(v) = s.check.crc32() {
                        f.push(s.offset, s.len, v);
                    }
                }
                let have = f
                    .range(unpacked_offset, len)
                    .ok_or(XzErrorKind::CheckMismatch)?;
                let want = u32::from_le_bytes(
                    stored
                        .get(..4)
                        .and_then(|s| s.try_into().ok())
                        .ok_or(XzErrorKind::TruncatedInput)?,
                );
                if have != want {
                    return Err(XzErrorKind::CheckMismatch);
                }
            }
            CheckType::Crc64 => {
                let mut f = CrcFolder::<u64>::new();
                for s in &segments {
                    if let Some(v) = s.check.crc64() {
                        f.push(s.offset, s.len, v);
                    }
                }
                let have = f
                    .range(unpacked_offset, len)
                    .ok_or(XzErrorKind::CheckMismatch)?;
                let want = u64::from_le_bytes(
                    stored
                        .get(..8)
                        .and_then(|s| s.try_into().ok())
                        .ok_or(XzErrorKind::TruncatedInput)?,
                );
                if have != want {
                    return Err(XzErrorKind::CheckMismatch);
                }
            }
            CheckType::Sha256 => {
                let have = digest.ok_or(XzErrorKind::UnsupportedCheck)?;
                let want = stored.get(..32).ok_or(XzErrorKind::TruncatedInput)?;
                if have != want {
                    return Err(XzErrorKind::CheckMismatch);
                }
            }
            CheckType::None | CheckType::Reserved(_) => {}
        }

        Ok(BlockCheck {
            unpacked_offset,
            len,
            segments,
            digest,
        })
    }
}
