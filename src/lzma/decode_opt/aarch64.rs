//! The arm64 fast decode loop.
//!
//! C: `Asm/arm64/LzmaDecOpt.S`, whose body is included verbatim as
//! `lzma_dec_opt_aarch64.S` next to this file; see the header of that file for
//! the three mechanical changes the translation needed.
//!
//! The loop is a whole function with its own prologue, epilogue and
//! callee-saved register handling, written to the platform's C calling
//! convention. A Rust *naked* function is therefore the exact fit: it emits
//! the body unchanged under a symbol Rust decorates for the target, with no
//! compiler-generated frame to conflict with the assembly's own. That is also
//! why this is not an `asm!` block inside an ordinary function — there is no
//! register allocation left for the compiler to do, and 1400 lines of
//! hand-allocated assembly cannot be handed register operands.

use core::arch::naked_asm;

use crate::lzma::decode::RealResult;
use crate::lzma::decode_opt::AsmLzmaDec;
use crate::lzma::state::LzmaDec;

/// C: `LzmaDec_DecodeReal_3(CLzmaDec *p, SizeT limit, const Byte *bufLimit)`.
///
/// Reads and writes `*p` at the offsets [`AsmLzmaDec`] pins down, takes the
/// input pointer from `p->buf` and leaves the advanced one there, and returns
/// `SZ_OK` (0) or `SZ_ERROR_DATA` (1).
///
/// # Safety
///
/// `p` must point to a fully initialised [`AsmLzmaDec`] whose `dic`, `probs`
/// and `buf` satisfy the decode-loop contract: `p->dicPos <= limit <=
/// p->dicBufSize`, `p->buf <= bufLimit`, and `LZMA_REQUIRED_INPUT_MAX`
/// readable bytes past `bufLimit`.
#[unsafe(naked)]
unsafe extern "C" fn lzma_dec_decode_real_3(
    p: *mut AsmLzmaDec,
    limit: usize,
    buf_limit: *const u8,
) -> u32 {
    naked_asm!(include_str!("lzma_dec_opt_aarch64.S"), options(raw))
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

    // SAFETY: three invariants, all established by the caller and carried
    // through this function's own contract.
    //
    // 1. State layout. `asm_state` is a `#[repr(C)]` mirror of `CLzmaDec`
    //    whose every offset is asserted at compile time against the
    //    `offset_*` constants in `LzmaDecOpt.S`, and it is fully initialised
    //    by `AsmLzmaDec::from_state`. Its `probs` and `dic` pointers borrow
    //    `p` for the duration of the call and nothing else aliases them,
    //    because `p` is a `&mut`.
    // 2. Input margin. `buf_start <= buf_limit` and the caller guarantees
    //    `LZMA_REQUIRED_INPUT_MAX` readable bytes past `buf_limit`, which is
    //    what lets the loop read ahead without checking; it is the same
    //    contract the portable loop relies on, stated in the assembly's own
    //    entry comment.
    // 3. Dictionary. `p.dic_pos <= limit <= p.dic_buf_size`, so `dic + limit`
    //    is one past the end of the dictionary at worst, and every match
    //    distance is checked against `checkDicSize` / `processedPos` inside
    //    the loop before it is used, exactly as in the C.
    //
    // The function is a complete C-ABI function: it saves and restores every
    // callee-saved register itself and uses the stack, so there are no
    // operands to declare and `nostack` would be wrong.
    let res = unsafe { lzma_dec_decode_real_3(&raw mut asm_state, limit, buf_limit) };

    asm_state.store_into(p);
    RealResult {
        buf: asm_state.buf,
        // C: `SZ_OK` / `SZ_ERROR_DATA`, the `mov sym, wzr` / `mov sym, 1`
        // that reach `fin`.
        ok: res == 0,
    }
}

#[cfg(test)]
mod tests {
    /// The assembly file's macros are named after x86 mnemonics, and `shl` is
    /// also a NEON instruction. An assembler macro outlives the file that
    /// defined it, and under LTO the whole program is one assembly unit, so a
    /// macro left defined shadows that mnemonic in every other crate's
    /// assembly. Whether it bites depends on the target and on emission order,
    /// which is why this reads the file instead of assembling a probe.
    #[test]
    fn the_assembly_file_purges_every_macro_it_defines() {
        // Without its `/* */` blocks: a definition inside one defines nothing,
        // and purging a macro that is not defined is an error.
        let mut source = String::new();
        let mut rest = include_str!("lzma_dec_opt_aarch64.S");
        while let Some(open) = rest.find("/*") {
            source.push_str(&rest[..open]);
            let close = rest[open..].find("*/").expect("an unclosed comment");
            rest = &rest[open + close + 2..];
        }
        source.push_str(rest);
        let named = |directive: &str| -> Vec<&str> {
            source
                .lines()
                .filter_map(|line| line.trim_start().strip_prefix(directive))
                .filter_map(|rest| rest.split_whitespace().next())
                .collect()
        };
        let defined = named(".macro ");
        let purged = named(".purgem ");
        assert!(!defined.is_empty());
        let left: Vec<_> = defined
            .iter()
            .filter(|name| !purged.contains(name))
            .collect();
        assert!(left.is_empty(), "macros left defined: {left:?}");
        let last_definition = source.rfind(".macro ").unwrap();
        let first_purge = source.find(".purgem ").unwrap();
        assert!(
            first_purge > last_definition,
            "a purge precedes a definition"
        );
    }
}
