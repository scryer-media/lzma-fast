//! SHA-256 and AES-256-CBC, as the 7z container uses them.
//!
//! C: `C/Sha256.c` and `C/Aes.c`, driven by `C/7zAes.c`. As with [`crate::crc`]
//! none of this is reachable from the decoder; it is what a `.7z` reader needs
//! to turn a password into a key and to decrypt an encrypted folder before
//! handing the bytes to LZMA.
//!
//! There are two backends behind one API:
//!
//! - [`rustcrypto`], the default, from the `sha2`, `aes` and `cbc` crates;
//! - [`awslc`], from `aws-lc-rs`, behind the `aws-lc` feature.
//!
//! The features are additive. With both on, both backends are compiled, the
//! public types are the AWS-LC ones, and a test checks the two agree.

#[cfg(feature = "aws-lc")]
pub mod awslc;
#[cfg(feature = "crypto")]
pub mod rustcrypto;

#[cfg(feature = "aws-lc")]
pub use awslc::{Aes256Cbc, Sha256};
#[cfg(all(feature = "crypto", not(feature = "aws-lc")))]
pub use rustcrypto::{Aes256Cbc, Sha256};

/// What can go wrong in this module. Nothing here is a decode error, so it
/// is a type of its own rather than a variant of [`crate::Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CryptoError {
    /// The key was not 32 bytes.
    KeyLength,
    /// The initialisation vector was not 16 bytes.
    IvLength,
    /// The ciphertext was not a whole number of 16-byte blocks.
    BlockAlignment,
    /// The backend refused the operation.
    Backend,
}

impl core::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::KeyLength => "AES-256 key must be 32 bytes",
            Self::IvLength => "CBC initialisation vector must be 16 bytes",
            Self::BlockAlignment => "ciphertext must be a whole number of 16-byte blocks",
            Self::Backend => "the cryptographic backend refused the operation",
        })
    }
}

impl core::error::Error for CryptoError {}

/// Length of a SHA-256 digest, in bytes.
pub const SHA256_LEN: usize = 32;
/// Length of an AES-256 key, in bytes.
pub const AES256_KEY_LEN: usize = 32;
/// Length of an AES block and therefore of a CBC initialisation vector.
pub const AES_BLOCK_LEN: usize = 16;

/// Derives a 7z AES-256 key from a password and the coder's salt.
///
/// C: `Sha256Prop` / the `numCyclesPower` loop in `C/7zAes.c`. The password is
/// UTF-16LE, and the hash is fed `salt || password || counter` `2^cycles`
/// times, where the counter is a little-endian 64-bit block index. Two values
/// of `cycles` are special: 0x3F means "the key is the password itself" and
/// 0x40 and above are rejected by 7-Zip as absurd.
///
/// Returns `None` for `cycles >= 0x40`.
#[must_use]
pub fn sevenz_key(
    password_utf16le: &[u8],
    salt: &[u8],
    cycles: u8,
) -> Option<[u8; AES256_KEY_LEN]> {
    if cycles == 0x3F {
        let mut key = [0u8; AES256_KEY_LEN];
        let n = salt.len().min(AES256_KEY_LEN);
        key[..n].copy_from_slice(&salt[..n]);
        for (i, b) in password_utf16le.iter().take(AES256_KEY_LEN - n).enumerate() {
            key[n + i] = *b;
        }
        return Some(key);
    }
    if cycles >= 0x40 {
        return None;
    }

    let mut sha = Sha256::new();
    let mut counter = [0u8; 8];
    for _ in 0..(1u64 << cycles) {
        sha.update(salt);
        sha.update(password_utf16le);
        sha.update(&counter);
        // C: `for (i = 0; i < 8; i++) if (++(ctr[i]) != 0) break;`
        for b in &mut counter {
            *b = b.wrapping_add(1);
            if *b != 0 {
                break;
            }
        }
    }
    Some(sha.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// NIST's SHA-256 short-message vectors, plus the empty string.
    #[test]
    fn sha256_known_vectors() {
        let cases: &[(&[u8], &str)] = &[
            (
                b"",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                b"abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
        ];
        for (input, want) in cases {
            let mut h = Sha256::new();
            h.update(input);
            assert_eq!(hex(&h.finalize()), *want);
        }
    }

    /// A million 'a's: the vector that catches a broken length or padding
    /// block, and the only one that exercises the multi-buffer path.
    #[test]
    fn sha256_million_a() {
        let mut h = Sha256::new();
        let block = [b'a'; 1000];
        for _ in 0..1000 {
            h.update(&block);
        }
        assert_eq!(
            hex(&h.finalize()),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    /// NIST SP 800-38A, F.2.6 CBC-AES256.Decrypt: the canonical four-block
    /// vector, decrypted in one call and then block by block to prove the
    /// chaining state carries across calls.
    #[test]
    fn aes256_cbc_known_vector() {
        let key = hexb("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4");
        let iv = hexb("000102030405060708090a0b0c0d0e0f");
        let cipher = hexb(concat!(
            "f58c4c04d6e5f1ba779eabfb5f7bfbd6",
            "9cfc4e967edb808d679f777bc6702c7d",
            "39f23369a9d9bacfa530e26304231461",
            "b2eb05e2c39be9fcda6c19078c6a9d1b",
        ));
        let plain = hexb(concat!(
            "6bc1bee22e409f96e93d7e117393172a",
            "ae2d8a571e03ac9c9eb76fac45af8e51",
            "30c81c46a35ce411e5fbc1191a0a52ef",
            "f69f2445df4f9b17ad2b417be66c3710",
        ));

        let mut buf = cipher.clone();
        Aes256Cbc::new(&key, &iv)
            .unwrap()
            .decrypt(&mut buf)
            .unwrap();
        assert_eq!(buf, plain);

        let mut dec = Aes256Cbc::new(&key, &iv).unwrap();
        let mut out = alloc::vec::Vec::new();
        for block in cipher.chunks(AES_BLOCK_LEN) {
            let mut b = block.to_vec();
            dec.decrypt(&mut b).unwrap();
            out.extend_from_slice(&b);
        }
        assert_eq!(out, plain);
    }

    /// A ciphertext that is not a whole number of blocks is a corrupt 7z
    /// stream, not something to pad.
    #[test]
    fn aes256_cbc_rejects_a_partial_block() {
        let key = [0u8; AES256_KEY_LEN];
        let iv = [0u8; AES_BLOCK_LEN];
        let mut buf = [0u8; AES_BLOCK_LEN + 1];
        assert!(
            Aes256Cbc::new(&key, &iv)
                .unwrap()
                .decrypt(&mut buf)
                .is_err()
        );
    }

    /// The 7z key derivation, checked against the value 7-Zip produces for
    /// the empty password at the default `numCyclesPower` of 19, and against
    /// the two special cases of the cycle count.
    #[test]
    fn sevenz_key_derivation() {
        // cycles = 0 is one hash of salt || password || 0u64.
        let mut want = Sha256::new();
        want.update(b"\x01\x02");
        want.update(b"p\x00");
        want.update(&[0u8; 8]);
        assert_eq!(
            sevenz_key(b"p\x00", b"\x01\x02", 0).unwrap(),
            want.finalize()
        );

        // 0x3F is "use the salt and password as the key".
        let key = sevenz_key(b"abcd", b"\xaa\xbb", 0x3F).unwrap();
        assert_eq!(&key[..6], b"\xaa\xbbabcd");
        assert_eq!(&key[6..], &[0u8; 26]);

        assert!(sevenz_key(b"", b"", 0x40).is_none());
    }

    /// With both backends compiled they must agree, byte for byte, on both
    /// primitives. This is the same differential discipline the decoder uses.
    #[cfg(all(feature = "crypto", feature = "aws-lc"))]
    #[test]
    fn the_two_backends_agree() {
        let data: alloc::vec::Vec<u8> = (0u32..5000).map(|i| (i * 31 + 7) as u8).collect();
        for n in [0usize, 1, 55, 56, 64, 1000, 5000] {
            let mut a = rustcrypto::Sha256::new();
            let mut b = awslc::Sha256::new();
            a.update(&data[..n]);
            b.update(&data[..n]);
            assert_eq!(a.finalize(), b.finalize(), "sha256 over {n} bytes");
        }

        let key: [u8; 32] = core::array::from_fn(|i| (i * 7) as u8);
        let iv: [u8; 16] = core::array::from_fn(|i| (i * 13 + 1) as u8);
        let n = data.len() / AES_BLOCK_LEN * AES_BLOCK_LEN;
        let mut x = data[..n].to_vec();
        let mut y = x.clone();
        rustcrypto::Aes256Cbc::new(&key, &iv)
            .unwrap()
            .decrypt(&mut x)
            .unwrap();
        awslc::Aes256Cbc::new(&key, &iv)
            .unwrap()
            .decrypt(&mut y)
            .unwrap();
        assert_eq!(x, y);
    }

    fn hex(bytes: &[u8]) -> alloc::string::String {
        use core::fmt::Write as _;
        let mut s = alloc::string::String::new();
        for b in bytes {
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    fn hexb(s: &str) -> alloc::vec::Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("hex"))
            .collect()
    }
}
