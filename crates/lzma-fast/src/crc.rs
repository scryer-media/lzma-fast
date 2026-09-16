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
}
