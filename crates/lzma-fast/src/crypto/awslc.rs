//! The AWS-LC backend, behind the `aws-lc` feature: `aws-lc-rs` over AWS-LC,
//! for callers who already have it in the build or who need its FIPS story.

use super::SHA256_LEN;

/// SHA-256. C: `C/Sha256.c`.
#[derive(Clone)]
pub struct Sha256(aws_lc_rs::digest::Context);

impl core::fmt::Debug for Sha256 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Sha256(..)")
    }
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    /// A hash over no bytes yet.
    #[must_use]
    pub fn new() -> Self {
        Self(aws_lc_rs::digest::Context::new(&aws_lc_rs::digest::SHA256))
    }

    /// Feeds the next bytes of the message.
    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    /// Consumes the hash and returns the digest.
    #[must_use]
    pub fn finalize(self) -> [u8; SHA256_LEN] {
        let digest = self.0.finish();
        let mut out = [0u8; SHA256_LEN];
        out.copy_from_slice(digest.as_ref());
        out
    }
}
