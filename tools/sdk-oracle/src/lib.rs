//! The LZMA SDK's own decoder, as an oracle.
//!
//! `build.rs` compiles `LzmaDec.c` and `Lzma2Dec.c` from a pinned SDK
//! checkout, once as C and, where the SDK's assembly builds, once with its
//! `LzmaDec_DecodeReal_3` loop. This crate wraps `LzmaDec_DecodeToBuf` and
//! `Lzma2Dec_DecodeToBuf` for each, which are the calls lzma-turbo's
//! `LzmaDecoder::decode` and `Lzma2Decoder::decode` port, so a test can drive
//! the SDK and the crate call for call and compare every step; `lockstep`
//! does that, for the tests here and for the differential fuzz target.

#![cfg_attr(not(oracle_c), allow(dead_code, unused_imports))]

use std::{
    ffi::{c_int, c_void},
    ptr::NonNull,
};

pub mod lockstep;

/// Which build of the SDK's decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// The SDK's C loop.
    C,
    /// The SDK's assembly loop, under the SDK's C driver.
    Asm,
}

/// The variants this build has: none without `LZMA_SDK`, the C one with it,
/// and the assembly one too on a target the SDK's assembly builds for.
pub const VARIANTS: &[Variant] = &[
    #[cfg(oracle_c)]
    Variant::C,
    #[cfg(oracle_asm)]
    Variant::Asm,
];

/// What one `*_DecodeToBuf` call did. `status` is the SDK's `ELzmaStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub read: usize,
    pub written: usize,
    pub status: u8,
}

/// An `SRes` other than `SZ_OK`.
pub type Code = i32;

pub const SZ_ERROR_DATA: Code = 1;
pub const SZ_ERROR_MEM: Code = 2;
pub const SZ_ERROR_UNSUPPORTED: Code = 4;
pub const SZ_ERROR_FAIL: Code = 11;

type NewLzma = unsafe extern "C" fn(*const u8, *mut c_int) -> *mut c_void;
type NewLzma2 = unsafe extern "C" fn(u8, *mut c_int) -> *mut c_void;
type Decode = unsafe extern "C" fn(
    *mut c_void,
    *mut u8,
    *mut usize,
    *const u8,
    *mut usize,
    c_int,
    *mut c_int,
) -> c_int;
type Free = unsafe extern "C" fn(*mut c_void);

struct Entry {
    lzma_new: NewLzma,
    lzma_decode: Decode,
    lzma_free: Free,
    lzma2_new: NewLzma2,
    lzma2_decode: Decode,
    lzma2_free: Free,
}

macro_rules! variant {
    ($cfg:ident, $name:ident, $lzma_new:ident, $lzma_decode:ident, $lzma_free:ident,
     $lzma2_new:ident, $lzma2_decode:ident, $lzma2_free:ident) => {
        #[cfg($cfg)]
        mod $name {
            use std::ffi::{c_int, c_void};

            unsafe extern "C" {
                pub fn $lzma_new(props: *const u8, res: *mut c_int) -> *mut c_void;
                pub fn $lzma_decode(
                    p: *mut c_void,
                    dest: *mut u8,
                    dest_len: *mut usize,
                    src: *const u8,
                    src_len: *mut usize,
                    finish_end: c_int,
                    status: *mut c_int,
                ) -> c_int;
                pub fn $lzma_free(p: *mut c_void);
                pub fn $lzma2_new(prop: u8, res: *mut c_int) -> *mut c_void;
                pub fn $lzma2_decode(
                    p: *mut c_void,
                    dest: *mut u8,
                    dest_len: *mut usize,
                    src: *const u8,
                    src_len: *mut usize,
                    finish_end: c_int,
                    status: *mut c_int,
                ) -> c_int;
                pub fn $lzma2_free(p: *mut c_void);
            }

            pub const ENTRY: super::Entry = super::Entry {
                lzma_new: $lzma_new,
                lzma_decode: $lzma_decode,
                lzma_free: $lzma_free,
                lzma2_new: $lzma2_new,
                lzma2_decode: $lzma2_decode,
                lzma2_free: $lzma2_free,
            };
        }
    };
}

variant!(
    oracle_c,
    c,
    oracle_c_lzma_new,
    oracle_c_lzma_decode,
    oracle_c_lzma_free,
    oracle_c_lzma2_new,
    oracle_c_lzma2_decode,
    oracle_c_lzma2_free
);
variant!(
    oracle_asm,
    asm,
    oracle_asm_lzma_new,
    oracle_asm_lzma_decode,
    oracle_asm_lzma_free,
    oracle_asm_lzma2_new,
    oracle_asm_lzma2_decode,
    oracle_asm_lzma2_free
);

fn entry(variant: Variant) -> &'static Entry {
    match variant {
        #[cfg(oracle_c)]
        Variant::C => &c::ENTRY,
        #[cfg(oracle_asm)]
        Variant::Asm => &asm::ENTRY,
        #[allow(unreachable_patterns)]
        _ => panic!("the SDK oracle was built without the {variant:?} variant"),
    }
}

/// One SDK decoder, LZMA or LZMA2.
pub struct Decoder {
    handle: NonNull<c_void>,
    decode: Decode,
    free: Free,
}

impl Decoder {
    /// `LzmaDec_Allocate` and `LzmaDec_Init` over the five property bytes of
    /// a `.lzma` header.
    pub fn lzma(variant: Variant, props: &[u8; 5]) -> Result<Self, Code> {
        let e = entry(variant);
        let mut res: c_int = 0;
        // SAFETY: `props` is the five bytes `LzmaDec_Allocate` reads.
        let handle = unsafe { (e.lzma_new)(props.as_ptr(), &mut res) };
        Self::wrap(handle, res, e.lzma_decode, e.lzma_free)
    }

    /// `Lzma2Dec_Allocate` and `Lzma2Dec_Init`.
    pub fn lzma2(variant: Variant, dict_prop: u8) -> Result<Self, Code> {
        let e = entry(variant);
        let mut res: c_int = 0;
        // SAFETY: plain value arguments.
        let handle = unsafe { (e.lzma2_new)(dict_prop, &mut res) };
        Self::wrap(handle, res, e.lzma2_decode, e.lzma2_free)
    }

    fn wrap(handle: *mut c_void, res: c_int, decode: Decode, free: Free) -> Result<Self, Code> {
        match NonNull::new(handle) {
            Some(handle) if res == 0 => Ok(Decoder {
                handle,
                decode,
                free,
            }),
            _ => Err(if res == 0 { SZ_ERROR_MEM } else { res }),
        }
    }

    /// `*_DecodeToBuf`, `LZMA_FINISH_END` when `finish_end`.
    pub fn decode(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        finish_end: bool,
    ) -> Result<Step, Code> {
        let mut read = input.len();
        let mut written = output.len();
        let mut status: c_int = 0;
        // SAFETY: the lengths passed are the slices' own, and the SDK reads
        // and writes no further than them.
        let res = unsafe {
            (self.decode)(
                self.handle.as_ptr(),
                output.as_mut_ptr(),
                &mut written,
                input.as_ptr(),
                &mut read,
                c_int::from(finish_end),
                &mut status,
            )
        };
        if res != 0 {
            return Err(res);
        }
        Ok(Step {
            read,
            written,
            status: u8::try_from(status).expect("ELzmaStatus"),
        })
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: the handle came from the matching `*_new` and is freed once.
        unsafe { (self.free)(self.handle.as_ptr()) }
    }
}
