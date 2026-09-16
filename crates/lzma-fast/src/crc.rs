//! The checksums the container formats around LZMA use.
//!
//! C: `C/7zCrc.c` (CRC-32, the 7z checksum) and `C/XzCrc64.c` (CRC-64/XZ,
//! the xz one). Neither is used by the decoder: they belong to the layer that
//! reads a container and verifies what came out of it.
//!
//! Both are carry-less-multiply implementations from [`crc_fast`], which is
//! the one library dependency this crate takes for them. Requires the `crc`
//! feature.

use crc_fast::{CrcAlgorithm, Digest};

/// CRC-32 (IEEE, reflected), the checksum in `.7z` headers and streams.
///
/// C: `CrcCalc` in `C/7zCrc.c`.
#[derive(Debug)]
pub struct Crc32(Digest);

/// CRC-64/XZ (ECMA-182, reflected, with the xz initial and final xor), the
/// checksum an `.xz` block or index carries.
///
/// C: `Crc64Calc` in `C/XzCrc64.c`.
#[derive(Debug)]
pub struct Crc64Xz(Digest);

macro_rules! digest {
    ($name:ident, $algo:expr, $out:ty, $one:ident, $doc:literal) => {
        impl $name {
            /// A digest over no bytes yet.
            #[must_use]
            pub fn new() -> Self {
                Self(Digest::new($algo))
            }

            /// Feeds the next bytes of the stream.
            pub fn update(&mut self, data: &[u8]) {
                self.0.update(data);
            }

            /// Consumes the digest and returns the checksum.
            #[must_use]
            pub fn finalize(self) -> $out {
                self.0.finalize() as $out
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        #[doc = $doc]
        #[must_use]
        pub fn $one(data: &[u8]) -> $out {
            crc_fast::checksum($algo, data) as $out
        }
    };
}

digest!(
    Crc32,
    CrcAlgorithm::Crc32IsoHdlc,
    u32,
    crc32,
    "CRC-32 of one contiguous buffer."
);
digest!(
    Crc64Xz,
    CrcAlgorithm::Crc64Xz,
    u64,
    crc64_xz,
    "CRC-64/XZ of one contiguous buffer."
);

// ---------------------------------------------------------------------------
// Folding
// ---------------------------------------------------------------------------

/// A CRC width that can be folded: given the checksums of two adjacent ranges
/// and the length of the second, the checksum of the two concatenated.
///
/// This is what makes a CRC usable from a decoder that produces its output out
/// of order, or on several threads at once: each worker checksums only the
/// bytes it produced, and the consumer folds the pieces together afterwards
/// without ever re-reading them. A cryptographic digest has no such operation,
/// which is why [`crate::crypto::Sha256`] is not foldable and the multi-threaded
/// decoder only ever offers it per whole block.
///
/// The two implementations are the two widths the containers around LZMA use,
/// and each width has exactly one algorithm here: `u32` is CRC-32/ISO-HDLC (the
/// 7z and xz CRC-32) and `u64` is CRC-64/XZ. There is no way to ask for a
/// different polynomial, because there is no other one to ask for.
pub trait Foldable: Copy + Eq {
    /// The checksum of an empty range, which is the identity for [`Self::fold`].
    const EMPTY: Self;

    /// The checksum of `a`'s bytes followed by `b`'s, where `b` covered
    /// `len_b` bytes.
    #[must_use]
    fn fold(a: Self, b: Self, len_b: u64) -> Self;
}

/// CRC-32/ISO-HDLC of `a`'s bytes followed by `b`'s.
///
/// `len_b` is the number of bytes `b` was computed over. `crc_a` may be the
/// checksum of any prefix, including the empty one (`0`).
#[must_use]
pub fn crc32_combine(crc_a: u32, crc_b: u32, len_b: u64) -> u32 {
    crc_fast::checksum_combine(
        CrcAlgorithm::Crc32IsoHdlc,
        u64::from(crc_a),
        u64::from(crc_b),
        len_b,
    ) as u32
}

/// CRC-64/XZ of `a`'s bytes followed by `b`'s.
///
/// `len_b` is the number of bytes `b` was computed over.
#[must_use]
pub fn crc64_xz_combine(crc_a: u64, crc_b: u64, len_b: u64) -> u64 {
    crc_fast::checksum_combine(CrcAlgorithm::Crc64Xz, crc_a, crc_b, len_b)
}

impl Foldable for u32 {
    const EMPTY: Self = 0;

    fn fold(a: Self, b: Self, len_b: u64) -> Self {
        crc32_combine(a, b, len_b)
    }
}

impl Foldable for u64 {
    const EMPTY: Self = 0;

    fn fold(a: Self, b: Self, len_b: u64) -> Self {
        crc64_xz_combine(a, b, len_b)
    }
}

/// Collects the checksums of pieces of a stream, in any order, and folds them
/// into the checksum of any contiguous range the pieces tile exactly.
///
/// This is the consumer half of worker-side checksumming. A decoder that hands
/// out `(offset, len, checksum)` for the pieces it produced - possibly out of
/// order, certainly on several threads - lets its caller ask for the checksum
/// of a file spanning several of them without buffering the bytes.
///
/// Pieces are kept as pushed and folded only on a query, deliberately. A CRC
/// cannot be un-folded: if adjacent pieces were combined eagerly, a later
/// question about a range ending inside the combined run would be unanswerable.
/// So a range can be answered exactly when the pushed pieces tile it, with a
/// piece boundary at each end - which is what happens when the split points
/// given to the decoder are the boundaries the consumer will ask about.
///
/// Pushing overlapping pieces is a caller error; the last one at an offset
/// wins and the folded answers are then meaningless.
#[derive(Debug, Clone)]
pub struct CrcFolder<W: Foldable> {
    pieces: alloc::collections::BTreeMap<u64, (u64, W)>,
}

impl<W: Foldable> Default for CrcFolder<W> {
    fn default() -> Self {
        Self::new()
    }
}

impl<W: Foldable> CrcFolder<W> {
    /// An empty folder.
    #[must_use]
    pub fn new() -> Self {
        CrcFolder {
            pieces: alloc::collections::BTreeMap::new(),
        }
    }

    /// Adds the checksum of the `len` bytes at `offset`. Zero-length pieces
    /// are ignored.
    pub fn push(&mut self, offset: u64, len: u64, checksum: W) {
        if len != 0 {
            self.pieces.insert(offset, (len, checksum));
        }
    }

    /// The checksum of `len` bytes at `offset`, or `None` if the pieces held
    /// do not tile that range exactly.
    ///
    /// An empty range is [`Foldable::EMPTY`] whether or not anything has been
    /// pushed.
    #[must_use]
    pub fn range(&self, offset: u64, len: u64) -> Option<W> {
        if len == 0 {
            return Some(W::EMPTY);
        }
        let end = offset.checked_add(len)?;
        let mut pos = offset;
        let mut acc = W::EMPTY;
        for (&s, &(l, c)) in self.pieces.range(offset..end) {
            if s != pos {
                return None;
            }
            acc = W::fold(acc, c, l);
            pos = s.checked_add(l)?;
            if pos >= end {
                break;
            }
        }
        if pos == end { Some(acc) } else { None }
    }

    /// How many pieces are held.
    #[must_use]
    pub fn pieces(&self) -> usize {
        self.pieces.len()
    }

    /// Forgets everything pushed so far.
    pub fn clear(&mut self) {
        self.pieces.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The check values both algorithms publish for the nine ASCII digits,
    /// plus the empty string, which is where an off-by-one in the initial or
    /// final xor shows up.
    #[test]
    fn known_vectors() {
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b"a"), 0xe8b7_be43);
        assert_eq!(crc64_xz(b""), 0x0000_0000_0000_0000);
        assert_eq!(crc64_xz(b"123456789"), 0x995d_c9bb_df19_39fa);
    }

    /// Splitting the stream must not change the answer; this is the property
    /// a container reader relies on when it checksums as it decodes.
    #[test]
    fn streaming_matches_one_shot() {
        let data: alloc::vec::Vec<u8> = (0u32..4096).map(|i| (i * 37 + 11) as u8).collect();
        for chunk in [1usize, 3, 64, 1000] {
            let mut a = Crc32::new();
            let mut b = Crc64Xz::new();
            for part in data.chunks(chunk) {
                a.update(part);
                b.update(part);
            }
            assert_eq!(a.finalize(), crc32(&data), "crc32, chunk {chunk}");
            assert_eq!(b.finalize(), crc64_xz(&data), "crc64, chunk {chunk}");
        }
    }
    /// Folding two adjacent pieces must give the one-shot checksum of the
    /// whole, at both widths. This is the property the multi-threaded decoder
    /// sells: a worker checksums only what it produced.
    #[test]
    fn combine_matches_one_shot() {
        let data: alloc::vec::Vec<u8> = (0u32..8192).map(|i| (i * 31 + 7) as u8).collect();
        for cut in [0usize, 1, 2, 17, 4096, 8191, 8192] {
            let (a, b) = data.split_at(cut);
            assert_eq!(
                crc32_combine(crc32(a), crc32(b), b.len() as u64),
                crc32(&data),
                "crc32, cut {cut}"
            );
            assert_eq!(
                crc64_xz_combine(crc64_xz(a), crc64_xz(b), b.len() as u64),
                crc64_xz(&data),
                "crc64, cut {cut}"
            );
        }
    }

    /// The empty checksum is the identity, which is what lets a folder start
    /// from nothing.
    #[test]
    fn empty_is_the_identity() {
        let data = b"the quick brown fox";
        assert_eq!(
            crc32_combine(u32::EMPTY, crc32(data), data.len() as u64),
            crc32(data)
        );
        assert_eq!(crc32_combine(crc32(data), u32::EMPTY, 0), crc32(data));
        assert_eq!(
            crc64_xz_combine(u64::EMPTY, crc64_xz(data), data.len() as u64),
            crc64_xz(data)
        );
        assert_eq!(
            crc64_xz_combine(crc64_xz(data), u64::EMPTY, 0),
            crc64_xz(data)
        );
    }

    /// Pieces pushed in any order fold to the one-shot checksum of every range
    /// they tile, and a range they do not tile is refused rather than guessed.
    #[test]
    fn folder_answers_tiled_ranges_only() {
        let data: alloc::vec::Vec<u8> = (0u32..1000).map(|i| (i * 13 + 5) as u8).collect();
        let bounds = [0usize, 1, 100, 101, 500, 999, 1000];

        let mut f32 = CrcFolder::<u32>::new();
        let mut f64 = CrcFolder::<u64>::new();
        // Pushed back to front, to prove order does not matter.
        for w in bounds.windows(2).rev() {
            let (a, b) = (w[0], w[1]);
            f32.push(a as u64, (b - a) as u64, crc32(&data[a..b]));
            f64.push(a as u64, (b - a) as u64, crc64_xz(&data[a..b]));
        }

        for (i, &a) in bounds.iter().enumerate() {
            for &b in &bounds[i..] {
                let len = (b - a) as u64;
                assert_eq!(
                    f32.range(a as u64, len),
                    Some(crc32(&data[a..b])),
                    "crc32 range {a}..{b}"
                );
                assert_eq!(
                    f64.range(a as u64, len),
                    Some(crc64_xz(&data[a..b])),
                    "crc64 range {a}..{b}"
                );
            }
        }

        // 50 is not a boundary, so neither end of a range at it can be folded.
        assert_eq!(f32.range(50, 50), None);
        assert_eq!(f32.range(0, 50), None);
        // Nor can a range run off the end of what has been pushed.
        assert_eq!(f32.range(0, 1001), None);

        // A hole in the middle is refused even though both sides are present.
        let mut holed = CrcFolder::<u32>::new();
        holed.push(0, 100, crc32(&data[..100]));
        holed.push(500, 100, crc32(&data[500..600]));
        assert_eq!(holed.range(0, 600), None);
        assert_eq!(holed.range(0, 100), Some(crc32(&data[..100])));
        assert_eq!(holed.pieces(), 2);
    }

    /// An empty range needs nothing pushed at all.
    #[test]
    fn folder_answers_the_empty_range() {
        let f = CrcFolder::<u32>::new();
        assert_eq!(f.range(0, 0), Some(0));
        assert_eq!(f.range(12345, 0), Some(0));
    }
}
