//! The AWS-LC backend, behind the `aws-lc` feature: `aws-lc-rs` over AWS-LC,
//! for callers who already have it in the build or who need its FIPS story.

use aws_lc_rs::cipher::{AES_256, DecryptingKey, DecryptionContext, UnboundCipherKey};
use aws_lc_rs::iv::FixedLength;

use super::{AES_BLOCK_LEN, AES256_KEY_LEN, CryptoError, SHA256_LEN};

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

/// Unpadded AES-256-CBC decryption. C: `C/Aes.c`, `AesCbc_Decode`.
///
/// AWS-LC's CBC API is one-shot, so the chaining state is carried here: the
/// initialisation vector of the next call is the last ciphertext block of
/// this one, which is what CBC chaining is.
#[derive(Clone)]
pub struct Aes256Cbc {
    key: [u8; AES256_KEY_LEN],
    iv: [u8; AES_BLOCK_LEN],
}

impl core::fmt::Debug for Aes256Cbc {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Never print key or chaining state.
        f.write_str("Aes256Cbc(..)")
    }
}

impl Aes256Cbc {
    /// A decryptor for `key` starting from `iv`.
    ///
    /// # Errors
    ///
    /// If `key` is not 32 bytes or `iv` is not 16.
    pub fn new(key: &[u8], iv: &[u8]) -> Result<Self, CryptoError> {
        Ok(Self {
            key: key.try_into().map_err(|_| CryptoError::KeyLength)?,
            iv: iv.try_into().map_err(|_| CryptoError::IvLength)?,
        })
    }

    /// Decrypts `data` in place and advances the chaining state.
    ///
    /// # Errors
    ///
    /// If `data` is not a whole number of 16-byte blocks, or if AWS-LC
    /// rejects the operation.
    pub fn decrypt(&mut self, data: &mut [u8]) -> Result<(), CryptoError> {
        if !data.len().is_multiple_of(AES_BLOCK_LEN) {
            return Err(CryptoError::BlockAlignment);
        }
        if data.is_empty() {
            return Ok(());
        }
        let next_iv: [u8; AES_BLOCK_LEN] = data[data.len() - AES_BLOCK_LEN..]
            .try_into()
            .expect("one block");

        let key = UnboundCipherKey::new(&AES_256, &self.key).map_err(|_| CryptoError::Backend)?;
        let key = DecryptingKey::cbc(key).map_err(|_| CryptoError::Backend)?;
        let context = DecryptionContext::Iv128(FixedLength::from(self.iv));
        key.decrypt(data, context)
            .map_err(|_| CryptoError::Backend)?;

        self.iv = next_iv;
        Ok(())
    }
}
