//! Assembly implementations of the fast decode loop.
//!
//! C: the `Z7_LZMA_DEC_OPT` path in `C/LzmaDec.c`, which replaces
//! `LzmaDec_DecodeReal_3` with the hand-written loop in `Asm/arm64/`
//! `LzmaDecOpt.S` or `Asm/x86/LzmaDecOpt.asm`. The portable port in
//! [`crate::lzma::decode`] stays as the fallback for every other target and
//! as the differential reference these loops are tested against.
//!
//! The assembly reads and writes `CLzmaDec` at hard-coded byte offsets, so
//! the state it is handed is a `#[repr(C)]` mirror of that structure rather
//! than this crate's own [`LzmaDec`], whose layout Rust is free to choose.
//! Filling the mirror in costs 96 bytes of copying per call to the loop,
//! which the caller makes about once per output buffer.

use crate::lzma::decode::RealResult;
use crate::lzma::state::LzmaDec;

#[cfg(all(feature = "asm", target_arch = "aarch64"))]
pub(crate) mod aarch64;

#[cfg(all(feature = "asm", target_arch = "x86_64"))]
pub(crate) mod x86_64;

/// True when a call to [`decode_real`] runs the assembly loop rather than the
/// portable one.
pub(crate) const ENABLED: bool = cfg!(all(
    feature = "asm",
    any(target_arch = "aarch64", target_arch = "x86_64")
));

/// C: `CLzmaDec` in `C/LzmaDec.h`, in the exact layout
/// `Asm/arm64/LzmaDecOpt.S` and `Asm/x86/LzmaDecOpt.asm` assume. Their
/// `offset_*` symbols are asserted against this below, as the assembly
/// asserts `offset_TOTAL_SIZE != 96` against its own copy.
#[cfg(all(feature = "asm", any(target_arch = "aarch64", target_arch = "x86_64")))]
#[repr(C)]
pub(crate) struct AsmLzmaDec {
    /// C: `CLzmaProps::lc`, at `offset_lc`.
    pub(crate) lc: u8,
    /// C: `CLzmaProps::lp`, at `offset_lp`.
    pub(crate) lp: u8,
    /// C: `CLzmaProps::pb`, at `offset_pb`.
    pub(crate) pb: u8,
    /// C: `CLzmaProps::_pad_`.
    pub(crate) pad: u8,
    /// C: `CLzmaProps::dicSize`, at `offset_dicSize`.
    pub(crate) dic_size: u32,
    /// C: `probs`, the base of the probability array, at `offset_probs`.
    pub(crate) probs: *mut u16,
    /// C: `probs_1664`, at `offset_probs_1664`. The assembly addresses the
    /// array from `probs` and never reads this, but the layout needs it.
    pub(crate) probs_1664: *mut u16,
    /// C: `dic`, at `offset_dic`.
    pub(crate) dic: *mut u8,
    /// C: `dicBufSize`, at `offset_dicBufSize`.
    pub(crate) dic_buf_size: usize,
    /// C: `dicPos`, at `offset_dicPos`.
    pub(crate) dic_pos: usize,
    /// C: `buf`, at `offset_buf`: the input pointer, in and out.
    pub(crate) buf: *const u8,
    /// C: `range`, at `offset_range`.
    pub(crate) range: u32,
    /// C: `code`, at `offset_code`.
    pub(crate) code: u32,
    /// C: `processedPos`, at `offset_processedPos`.
    pub(crate) processed_pos: u32,
    /// C: `checkDicSize`, at `offset_checkDicSize`.
    pub(crate) check_dic_size: u32,
    /// C: `reps[4]`, at `offset_rep0` .. `offset_rep3`.
    pub(crate) reps: [u32; 4],
    /// C: `state`, at `offset_state`.
    pub(crate) state: u32,
    /// C: `remainLen`, at `offset_remainLen`.
    pub(crate) remain_len: u32,
}

#[cfg(all(feature = "asm", any(target_arch = "aarch64", target_arch = "x86_64")))]
const _: () = {
    use core::mem::{offset_of, size_of};
    assert!(offset_of!(AsmLzmaDec, lc) == 0);
    assert!(offset_of!(AsmLzmaDec, lp) == 1);
    assert!(offset_of!(AsmLzmaDec, pb) == 2);
    assert!(offset_of!(AsmLzmaDec, dic_size) == 4);
    assert!(offset_of!(AsmLzmaDec, probs) == 8);
    assert!(offset_of!(AsmLzmaDec, probs_1664) == 16);
    assert!(offset_of!(AsmLzmaDec, dic) == 24);
    assert!(offset_of!(AsmLzmaDec, dic_buf_size) == 32);
    assert!(offset_of!(AsmLzmaDec, dic_pos) == 40);
    assert!(offset_of!(AsmLzmaDec, buf) == 48);
    assert!(offset_of!(AsmLzmaDec, range) == 56);
    assert!(offset_of!(AsmLzmaDec, code) == 60);
    assert!(offset_of!(AsmLzmaDec, processed_pos) == 64);
    assert!(offset_of!(AsmLzmaDec, check_dic_size) == 68);
    assert!(offset_of!(AsmLzmaDec, reps) == 72);
    assert!(offset_of!(AsmLzmaDec, state) == 88);
    assert!(offset_of!(AsmLzmaDec, remain_len) == 92);
    // C: `.if offset_TOTAL_SIZE != 96 / .error "Incorrect offset_TOTAL_SIZE"`.
    assert!(size_of::<AsmLzmaDec>() == 96);
};

#[cfg(all(feature = "asm", any(target_arch = "aarch64", target_arch = "x86_64")))]
impl AsmLzmaDec {
    /// Copies the parts of the decoder state the loop touches into the
    /// layout it expects.
    fn from_state(p: &mut LzmaDec, buf: *const u8) -> Self {
        let probs = p.probs.as_mut_ptr();
        AsmLzmaDec {
            lc: p.prop.lc(),
            lp: p.prop.lp(),
            pb: p.prop.pb(),
            pad: 0,
            dic_size: p.prop.dict_size(),
            probs,
            // SAFETY: the probability array is always at least
            // `NUM_BASE_PROBS == 1984` entries long, so `probs + 1664` is
            // inside it. The assembly never dereferences this field.
            probs_1664: unsafe { probs.add(crate::lzma::consts::K_START_OFFSET) },
            dic: p.dic.as_mut_ptr(),
            dic_buf_size: p.dic_buf_size,
            dic_pos: p.dic_pos,
            buf,
            range: p.range,
            code: p.code,
            processed_pos: p.processed_pos,
            check_dic_size: p.check_dic_size,
            reps: p.reps,
            state: p.state,
            remain_len: p.remain_len,
        }
    }

    /// Copies back what the loop changed. C: the `STORE_LZMA_*` block at
    /// `fin` in `LzmaDecOpt.S`.
    fn store_into(&self, p: &mut LzmaDec) {
        p.dic_pos = self.dic_pos;
        p.range = self.range;
        p.code = self.code;
        p.processed_pos = self.processed_pos;
        p.reps = self.reps;
        p.state = self.state;
        p.remain_len = self.remain_len;
    }
}

/// Runs the fast decode loop, in assembly where this build has one.
///
/// C: `LzmaDec_DecodeReal_3`, either the assembly entry point or the C
/// function of that name, exactly as `LZMA_DECODE_REAL` selects between them.
///
/// # Safety
///
/// Identical to [`crate::lzma::decode::lzma_dec_decode_real`]: `limit <=
/// p.dic_buf_size`, `p.dic_pos <= limit`, `buf_start <= buf_limit`, and at
/// least `LZMA_REQUIRED_INPUT_MAX` readable bytes past `buf_limit`.
#[inline]
pub(crate) unsafe fn decode_real(
    p: &mut LzmaDec,
    limit: usize,
    buf_start: *const u8,
    buf_limit: *const u8,
) -> RealResult {
    if ENABLED && !p.force_portable {
        #[cfg(all(feature = "asm", target_arch = "aarch64"))]
        // SAFETY: forwarded from this function's own contract, which is the
        // contract the assembly documents at its entry point.
        return unsafe { aarch64::decode_real(p, limit, buf_start, buf_limit) };

        #[cfg(all(feature = "asm", target_arch = "x86_64"))]
        // SAFETY: as above.
        return unsafe { x86_64::decode_real(p, limit, buf_start, buf_limit) };
    }
    // SAFETY: as above.
    unsafe { crate::lzma::decode::lzma_dec_decode_real(p, limit, buf_start, buf_limit) }
}
