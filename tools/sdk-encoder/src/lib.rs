//! The LZMA SDK's own encoder, as an oracle.
//!
//! `build.rs` compiles `LzmaEnc.c`, `Lzma2Enc.c`, the match finders behind
//! them and the two filters in front of them from a pinned SDK checkout, and
//! this wraps the four calls a differential test needs: encode LZMA-Alone,
//! encode raw LZMA2, delta, x86. The settings are the ones
//! `cargo xtask lzma-util`'s oracles take, so that a fuzz finding and a
//! failure in `tests/lzma_parity.rs` or `tests/lzma2_mt_parity.rs` describe
//! the same disagreement.
//!
//! Without `LZMA_SDK` there is no encoder here and [`AVAILABLE`] is `false`;
//! every call returns `None` and the caller says so rather than comparing the
//! crate against itself. CI sets `LZMA_ENCODER_ORACLE_REQUIRE=1`, which makes
//! the missing SDK a build failure instead.

use std::ffi::{c_int, c_uint};

/// Whether this build has the SDK's encoder in it.
pub const AVAILABLE: bool = cfg!(sdk_encoder);

/// The encoder settings, laid out as the wrapper's `SdkEncProps`.
///
/// `bt_mode` and `num_hash_bytes` are the SDK's two-field spelling of what
/// this crate calls a `MatchFinderKind`; `MatchFinderKind::bt_mode` and
/// `::num_hash_bytes` convert.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Props {
    pub level: c_int,
    pub bt_mode: c_int,
    pub num_hash_bytes: c_int,
    pub lc: c_int,
    pub lp: c_int,
    pub pb: c_int,
    pub fb: c_uint,
    pub dict_size: c_uint,
    /// The match finder's threads: 1, or 2 for `LzFindMt`.
    pub num_threads: c_int,
}

/// An `SRes` other than `SZ_OK`, as the SDK numbers them.
pub type Code = i32;

#[cfg(sdk_encoder)]
mod ffi {
    use std::ffi::c_int;

    unsafe extern "C" {
        pub fn sdk_enc_lzma1(
            src: *const u8,
            src_len: usize,
            props: *const super::Props,
            out: *mut *mut u8,
            out_len: *mut usize,
        ) -> c_int;
        pub fn sdk_enc_lzma2(
            src: *const u8,
            src_len: usize,
            props: *const super::Props,
            block_size: u64,
            block_threads: c_int,
            prop: *mut u8,
            out: *mut *mut u8,
            out_len: *mut usize,
        ) -> c_int;
        pub fn sdk_enc_free(p: *mut u8);
        pub fn sdk_filter_delta_enc(data: *mut u8, size: usize, delta: std::ffi::c_uint);
        pub fn sdk_filter_x86_enc(data: *mut u8, size: usize, pc: u32);
    }
}

/// Copy what the wrapper `malloc`ed, then hand it back to the C `free`.
///
/// # Safety
///
/// `ptr` must be the block the wrapper wrote through its out-parameter, of
/// `len` initialised bytes, not yet freed.
#[cfg(sdk_encoder)]
unsafe fn take(ptr: *mut u8, len: usize) -> Vec<u8> {
    if ptr.is_null() {
        return Vec::new();
    }
    // SAFETY: the wrapper reports `len` bytes written into `ptr`, which it
    // allocated in one block and has not freed; the copy ends before the
    // vector is handed over, so nothing borrows `ptr` afterwards.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
    // SAFETY: `ptr` came from the wrapper's `realloc`, is freed exactly here,
    // and is not used again.
    unsafe { ffi::sdk_enc_free(ptr) };
    bytes
}

/// The SDK's LZMA-Alone stream: 5 property bytes, the 8-byte uncompressed
/// size, then the coded data - what `lzma_turbo::encode_lzma_alone` produces.
///
/// `None` without the SDK, `Err` for an `SRes` the SDK refused the settings
/// with.
#[must_use = "the point is the bytes"]
pub fn lzma1(src: &[u8], props: &Props) -> Option<Result<Vec<u8>, Code>> {
    #[cfg(not(sdk_encoder))]
    {
        let _ = (src, props);
        None
    }
    #[cfg(sdk_encoder)]
    {
        let mut out: *mut u8 = std::ptr::null_mut();
        let mut len: usize = 0;
        // SAFETY: `src`/`props` are borrowed for the call and only read; the
        // two out-parameters are live locals the wrapper writes once, and it
        // writes them only when it returns `SZ_OK`.
        let res = unsafe { ffi::sdk_enc_lzma1(src.as_ptr(), src.len(), props, &mut out, &mut len) };
        if res != 0 {
            return Some(Err(res));
        }
        // SAFETY: `SZ_OK` means the wrapper wrote its block and `len` bytes.
        Some(Ok(unsafe { take(out, len) }))
    }
}

/// The SDK's raw LZMA2 stream and its single property byte - what
/// `lzma_turbo::encode_lzma2` produces.
///
/// `block_size` of 0 is solid, as the SDK's `LZMA2_ENC_PROPS_BLOCK_SIZE_SOLID`
/// is; `block_threads` above 1 is `MtCoder`, which splits at that block size.
#[must_use = "the point is the bytes"]
pub fn lzma2(
    src: &[u8],
    props: &Props,
    block_size: u64,
    block_threads: i32,
) -> Option<Result<(u8, Vec<u8>), Code>> {
    #[cfg(not(sdk_encoder))]
    {
        let _ = (src, props, block_size, block_threads);
        None
    }
    #[cfg(sdk_encoder)]
    {
        let mut out: *mut u8 = std::ptr::null_mut();
        let mut len: usize = 0;
        let mut prop: u8 = 0;
        // SAFETY: as above; `prop` is a live local the wrapper writes once.
        let res = unsafe {
            ffi::sdk_enc_lzma2(
                src.as_ptr(),
                src.len(),
                props,
                block_size,
                block_threads,
                &mut prop,
                &mut out,
                &mut len,
            )
        };
        if res != 0 {
            return Some(Err(res));
        }
        // SAFETY: `SZ_OK` means the wrapper wrote its block and `len` bytes.
        Some(Ok((prop, unsafe { take(out, len) })))
    }
}

/// The SDK's delta filter, encoding in place. `false` without the SDK.
pub fn delta_encode(data: &mut [u8], distance: u32) -> bool {
    #[cfg(not(sdk_encoder))]
    {
        let _ = (data, distance);
        false
    }
    #[cfg(sdk_encoder)]
    {
        // SAFETY: `Delta_Encode` reads and writes exactly `size` bytes from
        // the pointer, which is this slice, borrowed mutably for the call.
        unsafe { ffi::sdk_filter_delta_enc(data.as_mut_ptr(), data.len(), distance) };
        true
    }
}

/// The SDK's x86 branch converter, encoding in place. `false` without the SDK.
pub fn x86_encode(data: &mut [u8], pc: u32) -> bool {
    #[cfg(not(sdk_encoder))]
    {
        let _ = (data, pc);
        false
    }
    #[cfg(sdk_encoder)]
    {
        // SAFETY: the converter stays inside `size` bytes of the pointer,
        // which is this slice, borrowed mutably for the call.
        unsafe { ffi::sdk_filter_x86_enc(data.as_mut_ptr(), data.len(), pc) };
        true
    }
}
