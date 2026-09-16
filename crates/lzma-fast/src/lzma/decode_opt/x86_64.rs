//! The x86-64 fast decode loop.
//!
//! C: `Asm/x86/LzmaDecOpt.asm`, translated into GNU Intel syntax as
//! `lzma_dec_opt_x86_64_sysv.S` and `lzma_dec_opt_x86_64_win64.S` next to
//! this file; see the header of those files for what the translation had to
//! resolve. There are two of them because the reference file itself has two
//! calling conventions behind `ABI_LINUX`, and the assembly takes its
//! arguments in the ABI's own registers.
//!
//! As on aarch64 this is a naked function: the assembly is a whole C-ABI
//! function with its own prologue, epilogue, stack frame and callee-saved
//! register handling, so there is nothing left for the compiler to allocate.

use core::arch::naked_asm;

use crate::lzma::decode::RealResult;
use crate::lzma::decode_opt::AsmLzmaDec;
use crate::lzma::state::LzmaDec;

/// C: `LzmaDec_DecodeReal_3(CLzmaDec *p, SizeT limit, const Byte *bufLimit)`.
///
/// # Safety
///
/// See [`decode_real`]; `p` must be a fully initialised [`AsmLzmaDec`]
/// satisfying the decode-loop contract.
#[cfg(not(target_os = "windows"))]
#[unsafe(naked)]
unsafe extern "C" fn lzma_dec_decode_real_3(
    p: *mut AsmLzmaDec,
    limit: usize,
    buf_limit: *const u8,
) -> u32 {
    naked_asm!(include_str!("lzma_dec_opt_x86_64_sysv.S"), options(raw))
}

/// C: `LzmaDec_DecodeReal_3`, Microsoft x64 calling convention.
///
/// # Safety
///
/// See [`decode_real`].
#[cfg(target_os = "windows")]
#[unsafe(naked)]
unsafe extern "C" fn lzma_dec_decode_real_3(
    p: *mut AsmLzmaDec,
    limit: usize,
    buf_limit: *const u8,
) -> u32 {
    naked_asm!(include_str!("lzma_dec_opt_x86_64_win64.S"), options(raw))
}

/// C: `LZMA_DECODE_REAL` resolved to the assembly loop.
///
/// # Safety
///
/// See [`crate::lzma::decode_opt::decode_real`].
#[inline]
pub(crate) unsafe fn decode_real(
    p: &mut LzmaDec,
    limit: usize,
    buf_start: *const u8,
    buf_limit: *const u8,
) -> RealResult {
    let mut asm_state = AsmLzmaDec::from_state(p, buf_start);

    // SAFETY: the same three invariants as the aarch64 path, and for the same
    // reasons.
    //
    // 1. State layout. `asm_state` is a `#[repr(C)]` mirror of `CLzmaDec`
    //    whose offsets are asserted at compile time against the assembly's
    //    own `CLzmaDec_Asm` structure, and it is fully initialised. Its
    //    `probs` and `dic` pointers borrow `p`, which is a `&mut`, so nothing
    //    else aliases them.
    // 2. Input margin. `buf_start <= buf_limit` with
    //    `LZMA_REQUIRED_INPUT_MAX` readable bytes past `buf_limit`, which is
    //    what lets the loop read ahead unchecked.
    // 3. Dictionary. `p.dic_pos <= limit <= p.dic_buf_size`, and every match
    //    distance is checked against `checkDicSize` / `processedPos` inside
    //    the loop before it is used.
    //
    // The function is a complete C-ABI function that builds its own aligned
    // stack frame and saves every callee-saved register, so there are no
    // operands to declare and `nostack` would be wrong.
    let res = unsafe { lzma_dec_decode_real_3(&raw mut asm_state, limit, buf_limit) };

    asm_state.store_into(p);
    RealResult {
        buf: asm_state.buf,
        ok: res == 0,
    }
}
