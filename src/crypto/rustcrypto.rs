//! The RustCrypto backend: `sha2`. The default, and the one
//! that works on every target the decoder does.

use sha2::Digest as _;

use super::SHA256_LEN;

/// SHA-256. C: `C/Sha256.c`.
#[derive(Clone, Debug, Default)]
pub struct Sha256(sha2::Sha256);

impl Sha256 {
    /// A hash over no bytes yet.
    #[must_use]
    pub fn new() -> Self {
        Self(sha2::Sha256::new())
    }

    /// Feeds the next bytes of the message.
    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    /// Consumes the hash and returns the digest.
    #[must_use]
    pub fn finalize(self) -> [u8; SHA256_LEN] {
        self.0.finalize().into()
    }
}
