//! The RustCrypto backend: `sha2`, `aes` and `cbc`. The default, and the one
//! that works on every target the decoder does.

use aes::Aes256;
use aes::cipher::{BlockModeDecrypt, KeyIvInit};
use sha2::Digest as _;

use super::{AES_BLOCK_LEN, AES256_KEY_LEN, CryptoError, SHA256_LEN};

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

/// Unpadded AES-256-CBC decryption. C: `C/Aes.c`, `AesCbc_Decode`.
///
/// The chaining state carries across calls, so a caller may decrypt a stream
/// in whatever block-aligned pieces it has.
pub struct Aes256Cbc(cbc::Decryptor<Aes256>);

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
        let key: &[u8; AES256_KEY_LEN] = key.try_into().map_err(|_| CryptoError::KeyLength)?;
        let iv: &[u8; AES_BLOCK_LEN] = iv.try_into().map_err(|_| CryptoError::IvLength)?;
        Ok(Self(cbc::Decryptor::<Aes256>::new(key.into(), iv.into())))
    }

    /// Decrypts `data` in place and advances the chaining state.
    ///
    /// # Errors
    ///
    /// If `data` is not a whole number of 16-byte blocks, which in a 7z
    /// stream means the stream is corrupt.
    pub fn decrypt(&mut self, data: &mut [u8]) -> Result<(), CryptoError> {
        if !data.len().is_multiple_of(AES_BLOCK_LEN) {
            return Err(CryptoError::BlockAlignment);
        }
        for block in data.chunks_exact_mut(AES_BLOCK_LEN) {
            let block: &mut [u8; AES_BLOCK_LEN] = block.try_into().expect("exact chunk");
            self.0.decrypt_block(block.into());
        }
        Ok(())
    }
}
