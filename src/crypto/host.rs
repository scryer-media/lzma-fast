//! The host-delegated backend, behind the `crypto-host` feature: SHA-256 runs
//! in the embedding program, not in the guest.
//!
//! Selected on `wasm32` when `crypto-host` is on. wasm has no SHA extensions -
//! no `sha256rnds2`, no `sha256h` - so an in-guest SHA-256 is the plain 32-bit
//! compression function, byte after byte, while the host it is running inside
//! very likely has the instructions. Check type 10 covers a whole xz block, so
//! this is the one hash on the bulk path worth a boundary crossing.
//!
//! The delegated seam is a *streaming* one, because that is what the decoder
//! needs: a block arrives in whatever chunks the LZMA2 workers produce, and a
//! multi-gigabyte block must never be buffered whole just to be hashed. Each
//! [`Sha256`] owns one [`HostSha256Handle`] - an opaque embedder-side state -
//! from `new` until `finalize` or `Drop`, and `update` forwards the chunk it
//! was given. The full contract, including the handle-lifetime rules this type
//! keeps, is in [`crate::hooks`].
//!
//! The type presents exactly the API the other two backends do, `Clone`
//! included, so [`crate::mt::checksum`] and `crate::xz::check` see no
//! difference. `Clone` is the one method that needs the host's help: an opaque
//! handle cannot be copied by this crate, so it goes out to `sha256_clone`.
//!
//! This module compiles on native targets too, where it is not the active
//! backend, so that its handle lifetime can be exercised by the crate's own
//! tests against the reference hooks with no wasm runtime in sight.

use core::mem::ManuallyDrop;

use super::SHA256_LEN;
use crate::hooks::HostSha256Handle;

/// SHA-256, computed by the embedding host. C: `C/Sha256.c`.
///
/// Holds one live [`HostSha256Handle`] for its whole life. Every handle this
/// type opens is closed exactly once: by [`Sha256::finalize`], which takes the
/// digest, or by [`Drop`], which discards the state - so a host may treat a
/// handle it is never asked to release as a bug.
pub struct Sha256(HostSha256Handle);

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

impl Clone for Sha256 {
    /// A second hasher holding the same bytes so far.
    ///
    /// The handle is opaque, so this crate cannot duplicate the state itself;
    /// `sha256_clone` returns an independent live handle and the two hashers
    /// go their separate ways from here.
    fn clone(&self) -> Self {
        Self((crate::hooks::hooks().sha256_clone)(self.0))
    }
}

impl Sha256 {
    /// A hash over no bytes yet.
    ///
    /// # Panics
    ///
    /// If no hooks are installed. See [`crate::hooks`] for why that is a panic
    /// and not a fallback.
    #[must_use]
    pub fn new() -> Self {
        Self((crate::hooks::hooks().sha256_init)())
    }

    /// Feeds the next bytes of the message.
    pub fn update(&mut self, data: &[u8]) {
        (crate::hooks::hooks().sha256_update)(self.0, data);
    }

    /// Consumes the hash and returns the digest.
    #[must_use]
    pub fn finalize(self) -> [u8; SHA256_LEN] {
        // `sha256_finalize` consumes the handle, so `Drop` must not then hand
        // the same value to `sha256_drop`: that is the double release the
        // contract forbids, and the one thing a host cannot defend against.
        let this = ManuallyDrop::new(self);
        (crate::hooks::hooks().sha256_finalize)(this.0)
    }
}

impl Drop for Sha256 {
    /// Releases the handle for a hasher that was never finalized - a block
    /// whose decode failed, or a plan that turned out not to need the digest.
    fn drop(&mut self) {
        (crate::hooks::hooks().sha256_drop)(self.0);
    }
}

// ===========================================================================
// NATIVE test: the handle lifetime this type promises the host, proven without
// a wasm runtime. The hooks are plain `fn` pointers, so installing the
// reference set makes this the real delegation path - init, clone, update,
// finalize, drop - and not a simulation of it.
// ===========================================================================
#[cfg(all(
    test,
    not(target_family = "wasm"),
    feature = "crc",
    any(feature = "crypto", feature = "native-crypto")
))]
mod tests {
    use super::*;

    /// The in-process backend this build's `crypto::Sha256` resolves to, which
    /// is the ground truth the delegated one must equal byte for byte.
    #[cfg(feature = "native-crypto")]
    type Reference = super::super::rustcrypto::Sha256;
    #[cfg(all(feature = "crypto", not(feature = "native-crypto")))]
    type Reference = super::super::awslc::Sha256;

    fn reference(chunks: &[&[u8]]) -> [u8; SHA256_LEN] {
        let mut h = Reference::new();
        for c in chunks {
            h.update(c);
        }
        h.finalize()
    }

    /// Streaming through the host in arbitrary chunks equals the one-shot
    /// digest - the property the xz check depends on, since a block reaches
    /// the hasher in whatever pieces the decoder produced.
    #[test]
    fn host_sha256_streams_like_the_in_process_backend() {
        crate::hooks::install_reference_hooks_for_test();

        let data: alloc::vec::Vec<u8> = (0u32..5000).map(|i| (i * 31 + 7) as u8).collect();
        for chunk in [1usize, 3, 55, 56, 64, 1000, 5000, 8192] {
            let mut host = Sha256::new();
            let mut want = Reference::new();
            for part in data.chunks(chunk) {
                host.update(part);
                want.update(part);
            }
            assert_eq!(
                host.finalize(),
                want.finalize(),
                "host sha256 diverged at chunk size {chunk}"
            );
        }

        // The empty message, and an empty update in the middle of a stream,
        // are both legal and must change nothing.
        assert_eq!(Sha256::new().finalize(), reference(&[]));
        let mut h = Sha256::new();
        h.update(b"abc");
        h.update(&[]);
        h.update(b"def");
        assert_eq!(h.finalize(), reference(&[b"abc", b"def"]));
    }

    /// A clone is independent in both directions, and a hasher that is dropped
    /// instead of finalized releases its handle rather than leaking it. The
    /// reference hooks back each handle with a leaked `Box`, so a double
    /// release or a use-after-finalize here would be caught by the test
    /// runner's allocator, not merely by a wrong digest.
    #[test]
    fn host_sha256_clone_is_independent_and_drop_releases() {
        crate::hooks::install_reference_hooks_for_test();

        let mut a = Sha256::new();
        a.update(b"abc");
        let mut b = a.clone();
        a.update(b"def");
        b.update(b"xyz");

        assert_eq!(a.finalize(), reference(&[b"abc", b"def"]));
        assert_eq!(b.finalize(), reference(&[b"abc", b"xyz"]));

        // Dropped without a digest: the handle must still be released.
        let mut never_read = Sha256::new();
        never_read.update(b"this digest is never taken");
        drop(never_read);

        // And a clone dropped while its original lives on.
        let mut original = Sha256::new();
        original.update(b"hello ");
        drop(original.clone());
        original.update(b"world");
        assert_eq!(original.finalize(), reference(&[b"hello ", b"world"]));
    }
}
