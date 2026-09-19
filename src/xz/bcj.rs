//! The branch/call/jump converters, both directions.
//!
//! C: `C/Bra86.c` (`z7_BranchConvSt_X86_Dec` / `_Enc`) and `C/Bra.c`
//! (`z7_BranchConv_{ARM64,ARM,ARMT,PPC,SPARC,IA64}_Dec` / `_Enc` and
//! `z7_BranchConv_RISCV_Dec` / `_Enc`), both public domain. These are ports, goto for goto: the x86 one in particular is not
//! the obvious byte-at-a-time filter but a four-byte scan that tests all four
//! positions of a word at once, which is why it is worth porting rather than
//! rewriting.
//!
//! Every converter has the same contract as the C:
//!
//! - it converts in place and returns how many bytes it processed;
//! - the caller keeps the unprocessed tail and passes it again, prefixed to
//!   the next bytes, with `pc` advanced by what was processed;
//! - a return of zero means "not enough data yet", which is allowed whenever
//!   the buffer is shorter than the filter's alignment plus its lookahead.
//!
//! The x86 converter additionally carries three bits of state between calls.
//!
//! Encoding and decoding are the same function in the C, which takes an
//! `encoding` flag and expands `BR_CONVERT_VAL(v, c)` to `v += c` or
//! `v -= c`; that is the whole difference for every converter but IA64, which
//! masks its `pc` differently, and RISC-V, which the C writes out as two
//! separate functions. This port keeps that shape: one `*_conv` per converter
//! taking the flag, with `*_decode` and `*_encode` wrappers, and RISC-V's two
//! functions written out separately as the C does.
//!
//! These are `pub` on purpose: the 7z crate that sits on this one needs the
//! same converters, and there should be one copy of them.

use super::error::XzErrorKind;

// ---------------------------------------------------------------------------
// Little helpers. `Ui` is little-endian in the C, `Be` big-endian, whatever
// the host is; `GetUi32a` only differs from `GetUi32` in alignment, which
// Rust does not need told.
// ---------------------------------------------------------------------------

#[inline]
fn get_u32le(d: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]])
}

#[inline]
fn set_u32le(d: &mut [u8], i: usize, v: u32) {
    d[i..i + 4].copy_from_slice(&v.to_le_bytes());
}

#[inline]
fn get_u32be(d: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]])
}

#[inline]
fn set_u32be(d: &mut [u8], i: usize, v: u32) {
    d[i..i + 4].copy_from_slice(&v.to_be_bytes());
}

#[inline]
fn get_u16le(d: &[u8], i: usize) -> u32 {
    u32::from(u16::from_le_bytes([d[i], d[i + 1]]))
}

#[inline]
fn set_u16le(d: &mut [u8], i: usize, v: u16) {
    d[i..i + 2].copy_from_slice(&v.to_le_bytes());
}

/// C: `BR_CONVERT_VAL(v, c)`, the one line that separates an encoder from a
/// decoder in every converter but RISC-V's.
#[inline]
fn convert_val(v: u32, c: u32, encoding: bool) -> u32 {
    if encoding {
        v.wrapping_add(c)
    } else {
        v.wrapping_sub(c)
    }
}

/// Which converter, and with it the alignment a start offset must respect and
/// the lookahead the caller must leave.
///
/// Spec §5.3.2 for the ids and alignments; the lookahead column is from
/// `Bra.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BcjKind {
    /// Filter id 0x04.
    X86,
    /// Filter id 0x05, big-endian PowerPC.
    Ppc,
    /// Filter id 0x06.
    Ia64,
    /// Filter id 0x07, little-endian instruction encoding.
    Arm,
    /// Filter id 0x08, little-endian instruction encoding.
    ArmThumb,
    /// Filter id 0x09, big-endian SPARC.
    Sparc,
    /// Filter id 0x0A.
    Arm64,
    /// Filter id 0x0B, xz 5.6 and later.
    RiscV,
}

impl BcjKind {
    /// The filter id this converter is stored as in a block header.
    #[must_use]
    pub fn filter_id(self) -> u64 {
        match self {
            BcjKind::X86 => 0x04,
            BcjKind::Ppc => 0x05,
            BcjKind::Ia64 => 0x06,
            BcjKind::Arm => 0x07,
            BcjKind::ArmThumb => 0x08,
            BcjKind::Sparc => 0x09,
            BcjKind::Arm64 => 0x0A,
            BcjKind::RiscV => 0x0B,
        }
    }

    /// The converter for a filter id, if it is one of them.
    #[must_use]
    pub fn from_filter_id(id: u64) -> Option<Self> {
        Some(match id {
            0x04 => BcjKind::X86,
            0x05 => BcjKind::Ppc,
            0x06 => BcjKind::Ia64,
            0x07 => BcjKind::Arm,
            0x08 => BcjKind::ArmThumb,
            0x09 => BcjKind::Sparc,
            0x0A => BcjKind::Arm64,
            0x0B => BcjKind::RiscV,
            _ => return None,
        })
    }

    /// The alignment a start offset must be a multiple of. Spec §5.3.2 makes
    /// a misaligned start offset an error rather than something to round.
    #[must_use]
    pub fn alignment(self) -> u32 {
        match self {
            BcjKind::X86 => 1,
            BcjKind::ArmThumb | BcjKind::RiscV => 2,
            BcjKind::Ppc | BcjKind::Arm | BcjKind::Sparc | BcjKind::Arm64 => 4,
            BcjKind::Ia64 => 16,
        }
    }

    /// Bytes past the alignment the converter must be able to see before it
    /// will convert an instruction. C: the LookAhead column in `Bra.h`.
    #[must_use]
    pub fn lookahead(self) -> usize {
        match self {
            BcjKind::X86 => 4,
            BcjKind::ArmThumb => 2,
            BcjKind::RiscV => 6,
            _ => 0,
        }
    }

    /// The most bytes a converter can leave unprocessed at the end of a
    /// buffer, and so the most a caller has to carry to the next call.
    #[must_use]
    pub fn max_carry(self) -> usize {
        self.alignment() as usize + self.lookahead() - 1
    }
}

/// One BCJ converter, with the state it carries between calls.
#[derive(Debug, Clone, Copy)]
pub struct Bcj {
    kind: BcjKind,
    /// C: the `pc` parameter, advanced by the caller after every call.
    pc: u32,
    /// C: `*state`, used by the x86 converter alone.
    state: u32,
}

impl Bcj {
    /// A converter starting at virtual address `start_offset`.
    ///
    /// # Errors
    ///
    /// [`XzErrorKind::BadFilterChain`] if `start_offset` is not a multiple of
    /// the converter's alignment (spec §5.3.2).
    pub fn new(kind: BcjKind, start_offset: u32) -> Result<Self, XzErrorKind> {
        if !start_offset.is_multiple_of(kind.alignment()) {
            return Err(XzErrorKind::BadFilterChain);
        }
        Ok(Bcj {
            kind,
            pc: start_offset,
            // C: Z7_BRANCH_CONV_ST_X86_STATE_INIT_VAL.
            state: 0,
        })
    }

    /// Which converter this is.
    #[must_use]
    pub fn kind(&self) -> BcjKind {
        self.kind
    }

    /// Converts in place and returns how many leading bytes were converted.
    ///
    /// The tail beyond the returned length is untouched and must be offered
    /// again at the start of the next call.
    pub fn decode(&mut self, data: &mut [u8]) -> usize {
        let n = match self.kind {
            BcjKind::X86 => x86_decode(data, self.pc, &mut self.state),
            BcjKind::Ppc => ppc_decode(data, self.pc),
            BcjKind::Ia64 => ia64_decode(data, self.pc),
            BcjKind::Arm => arm_decode(data, self.pc),
            BcjKind::ArmThumb => armt_decode(data, self.pc),
            BcjKind::Sparc => sparc_decode(data, self.pc),
            BcjKind::Arm64 => arm64_decode(data, self.pc),
            BcjKind::RiscV => riscv_decode(data, self.pc),
        };
        self.pc = self.pc.wrapping_add(n as u32);
        n
    }

    /// The other direction, with the same contract: converts in place, returns
    /// how many leading bytes were converted, and leaves the tail for the next
    /// call. Feeding a buffer in pieces therefore gives the same bytes as
    /// feeding it whole, exactly as on the decode side.
    pub fn encode(&mut self, data: &mut [u8]) -> usize {
        let n = match self.kind {
            BcjKind::X86 => x86_encode(data, self.pc, &mut self.state),
            BcjKind::Ppc => ppc_encode(data, self.pc),
            BcjKind::Ia64 => ia64_encode(data, self.pc),
            BcjKind::Arm => arm_encode(data, self.pc),
            BcjKind::ArmThumb => armt_encode(data, self.pc),
            BcjKind::Sparc => sparc_encode(data, self.pc),
            BcjKind::Arm64 => arm64_encode(data, self.pc),
            BcjKind::RiscV => riscv_encode(data, self.pc),
        };
        self.pc = self.pc.wrapping_add(n as u32);
        n
    }
}

// ---------------------------------------------------------------------------
// The two directions of each converter. C: the `Z7_BRANCH_CONV_*_FUNC_IMP`
// macros, which instantiate each shared body with `encoding` 0 and 1.
// ---------------------------------------------------------------------------

macro_rules! branch_funcs {
    ($conv:ident, $dec:ident, $enc:ident, $cname:literal) => {
        #[doc = concat!("C: `z7_BranchConv_", $cname, "_Dec`.")]
        #[must_use]
        pub fn $dec(data: &mut [u8], pc: u32) -> usize {
            $conv(data, pc, false)
        }

        #[doc = concat!("C: `z7_BranchConv_", $cname, "_Enc`.")]
        #[must_use]
        pub fn $enc(data: &mut [u8], pc: u32) -> usize {
            $conv(data, pc, true)
        }
    };
}

branch_funcs!(arm64_conv, arm64_decode, arm64_encode, "ARM64");
branch_funcs!(arm_conv, arm_decode, arm_encode, "ARM");
branch_funcs!(ppc_conv, ppc_decode, ppc_encode, "PPC");
branch_funcs!(sparc_conv, sparc_decode, sparc_encode, "SPARC");
branch_funcs!(armt_conv, armt_decode, armt_encode, "ARMT");
branch_funcs!(ia64_conv, ia64_decode, ia64_encode, "IA64");

/// C: `z7_BranchConvSt_X86_Dec`.
#[must_use]
pub fn x86_decode(data: &mut [u8], pc: u32, state: &mut u32) -> usize {
    x86_conv(data, pc, state, false)
}

/// C: `z7_BranchConvSt_X86_Enc`.
#[must_use]
pub fn x86_encode(data: &mut [u8], pc: u32, state: &mut u32) -> usize {
    x86_conv(data, pc, state, true)
}

// ---------------------------------------------------------------------------
// x86. C: Bra86.c, z7_BranchConvSt_X86_Dec.
// ---------------------------------------------------------------------------

/// C: `BR86_NEED_CONV_FOR_MS_BYTE(b)`.
#[inline]
fn need_conv_ms_byte(b: u32) -> bool {
    ((b as u8).wrapping_add(1)) & 0xFE == 0
}

/// C: `BR86_IS_BCJ_BYTE(n)` over `v = GetUi32(p) ^ 0xe8e8e8e8`.
#[inline]
fn is_bcj_byte(v: u32, n: u32) -> bool {
    v & (0xFEu32 << (n * 8)) == 0
}

/// Where the transcription of the C's `goto` graph currently is.
enum X86Label {
    /// C: `for (;; mask |= 4)`, the loop header.
    Cont,
    /// C: `start:`.
    Start,
    /// C: the tail of `m0`/`m1`/`m2`.
    Mid,
    /// C: `main_loop:`.
    MainLoop,
    /// C: `a3:`.
    A3,
}

/// C: `z7_BranchConvSt_X86_Dec`, which is `Z7_BRANCH_CONV_ST(X86)` with
/// `encoding = 0`.
///
/// Ported with the `goto` graph made explicit rather than restructured: the
/// four-way word scan and the mask bookkeeping only make sense together.
#[must_use]
fn x86_conv(data: &mut [u8], pc: u32, state: &mut u32, encoding: bool) -> usize {
    let size = data.len();
    if size < 5 {
        return 0;
    }
    let lim = size - 4;
    let mut mask = *state;
    let mut p = 0usize;
    // C: `pc += 4; BR_PC_INIT`, so BR_PC_GET at index `p` is `pc + 4 + p`.
    let pc4 = pc.wrapping_add(4);

    let mut label = X86Label::Start;
    let p = loop {
        match label {
            X86Label::Cont => {
                mask |= 4;
                label = X86Label::Start;
            }
            X86Label::Start => {
                if p >= lim {
                    break p;
                }
                let v = get_u32le(data, p) ^ 0xE8E8_E8E8;
                p += 4;
                if is_bcj_byte(v, 0) {
                    p -= 3;
                    label = X86Label::Mid;
                    continue;
                }
                mask >>= 1;
                if is_bcj_byte(v, 1) {
                    p -= 2;
                    label = X86Label::Mid;
                    continue;
                }
                mask >>= 1;
                if is_bcj_byte(v, 2) {
                    p -= 1;
                    label = X86Label::Mid;
                    continue;
                }
                mask = 0;
                label = if is_bcj_byte(v, 3) {
                    X86Label::A3
                } else {
                    X86Label::MainLoop
                };
            }
            X86Label::Mid => {
                if mask == 0 {
                    label = X86Label::A3;
                    continue;
                }
                if p > lim {
                    p -= 1;
                    break p;
                }
                // C: `if (mask > 4 || mask == 3)`, i.e. the masks that say a
                // conversion here would overlap one already made.
                if mask > 4 || mask == 3 {
                    mask >>= 1;
                    label = X86Label::Cont;
                    continue;
                }
                mask >>= 1;
                if need_conv_ms_byte(u32::from(data[p + mask as usize])) {
                    label = X86Label::Cont;
                    continue;
                }
                let mut v = get_u32le(data, p);
                v = v.wrapping_add(1 << 24);
                if v & 0xFE00_0000 != 0 {
                    label = X86Label::Cont;
                    continue;
                }
                let c = pc4.wrapping_add(p as u32);
                v = convert_val(v, c, encoding);
                let sh = mask << 3;
                if need_conv_ms_byte(v >> sh) {
                    v ^= (0x100u32 << sh).wrapping_sub(1);
                    v = convert_val(v, c, encoding);
                }
                mask = 0;
                v &= (1 << 25) - 1;
                v = v.wrapping_sub(1 << 24);
                set_u32le(data, p, v);
                p += 4;
                label = X86Label::MainLoop;
            }
            X86Label::MainLoop => {
                if p >= lim {
                    break p;
                }
                let hit = loop {
                    let v = get_u32le(data, p) ^ 0xE8E8_E8E8;
                    p += 4;
                    if is_bcj_byte(v, 0) {
                        p -= 3;
                        break true;
                    }
                    if is_bcj_byte(v, 1) {
                        p -= 2;
                        break true;
                    }
                    if is_bcj_byte(v, 2) {
                        p -= 1;
                        break true;
                    }
                    if is_bcj_byte(v, 3) {
                        break true;
                    }
                    if p >= lim {
                        break false;
                    }
                };
                if !hit {
                    break p;
                }
                label = X86Label::A3;
            }
            X86Label::A3 => {
                if p > lim {
                    p -= 1;
                    break p;
                }
                let mut v = get_u32le(data, p);
                v = v.wrapping_add(1 << 24);
                if v & 0xFE00_0000 != 0 {
                    label = X86Label::Cont;
                    continue;
                }
                let c = pc4.wrapping_add(p as u32);
                v = convert_val(v, c, encoding);
                v &= (1 << 25) - 1;
                v = v.wrapping_sub(1 << 24);
                set_u32le(data, p, v);
                p += 4;
                label = X86Label::MainLoop;
            }
        }
    };

    *state = mask;
    p
}

// ---------------------------------------------------------------------------
// The RISC converters. C: Bra.c.
// ---------------------------------------------------------------------------

/// C: `z7_BranchConv_ARM64_Dec` / `_Enc`.
fn arm64_conv(data: &mut [u8], pc: u32, encoding: bool) -> usize {
    const FLAG: u32 = 1 << (24 - 4);
    const MASK: u32 = (1 << 24) - (FLAG << 1);
    let lim = data.len() & !3;
    let mut j = 0usize;
    while j != lim {
        let mut v = get_u32le(data, j);
        // C: `pc -= 4` before the loop and `p` points past the instruction,
        // so BR_PC_GET for the instruction at `j` is `pc + j`.
        let pc_here = pc.wrapping_add(j as u32);
        if v.wrapping_sub(0x9400_0000) & 0xFC00_0000 == 0 {
            let c = pc_here >> 2;
            v = convert_val(v, c, encoding);
            v &= 0x03FF_FFFF;
            v |= 0x9400_0000;
            set_u32le(data, j, v);
            j += 4;
            continue;
        }
        v = v.wrapping_sub(0x9000_0000);
        if v & 0x9F00_0000 == 0 {
            v = v.wrapping_add(FLAG);
            if v & MASK != 0 {
                j += 4;
                continue;
            }
            let mut z = (v & 0xFFFF_FFE0) | (v >> 26);
            let c = (pc_here >> (12 - 3)) & !7u32;
            z = convert_val(z, c, encoding);
            v &= 0x1F;
            v |= 0x9000_0000;
            v |= z << 26;
            v |= 0x00FF_FFE0 & (z & ((FLAG << 1) - 1)).wrapping_sub(FLAG);
            set_u32le(data, j, v);
        }
        j += 4;
    }
    lim
}

/// C: `z7_BranchConv_ARM_Dec` / `_Enc`.
fn arm_conv(data: &mut [u8], pc: u32, encoding: bool) -> usize {
    let lim = data.len() & !3;
    let mut j = 0usize;
    while j != lim {
        if data[j + 3] == 0xEB {
            let v = get_u32le(data, j);
            // C: `pc += 8 - 4` and `p` points past the instruction: an ARM
            // branch offset is relative to the instruction after next.
            let c = pc.wrapping_add(j as u32).wrapping_add(8) >> 2;
            let mut v = convert_val(v, c, encoding);
            v &= 0x00FF_FFFF;
            v |= 0xEB00_0000;
            set_u32le(data, j, v);
        }
        j += 4;
    }
    lim
}

/// C: `z7_BranchConv_PPC_Dec` / `_Enc`.
fn ppc_conv(data: &mut [u8], pc: u32, encoding: bool) -> usize {
    let lim = data.len() & !3;
    let mut j = 0usize;
    while j != lim {
        let v = get_u32be(data, j);
        if v.wrapping_sub(0x4800_0001) & 0xFC00_0003 == 0 {
            let c = pc.wrapping_add(j as u32);
            let mut v = convert_val(v, c, encoding);
            v &= 0x03FF_FFFF;
            v |= 0x4800_0000;
            set_u32be(data, j, v);
        }
        j += 4;
    }
    lim
}

/// C: `z7_BranchConv_SPARC_Dec` / `_Enc`, the branch without
/// `BR_SPARC_USE_ROTATE`.
fn sparc_conv(data: &mut [u8], pc: u32, encoding: bool) -> usize {
    const FLAG: u32 = 1 << 22;
    let lim = data.len() & !3;
    let mut j = 0usize;
    while j != lim {
        let mut v = get_u32be(data, j);
        v = v.wrapping_add(5 << 29);
        v ^= 7 << 29;
        v = v.wrapping_add(FLAG);
        if v & (FLAG << 1).wrapping_neg() == 0 {
            v <<= 2;
            let c = pc.wrapping_add(j as u32);
            v = convert_val(v, c, encoding);
            v &= (FLAG << 3) - 1;
            v = v.wrapping_sub(FLAG << 2);
            v >>= 2;
            v |= 1 << 30;
            set_u32be(data, j, v);
        }
        j += 4;
    }
    lim
}

/// C: `z7_BranchConv_ARMT_Dec` / `_Enc`.
fn armt_conv(data: &mut [u8], pc: u32, encoding: bool) -> usize {
    let size = data.len() & !1;
    if size <= 2 {
        return 0;
    }
    // C: `size -= 2`, so `lim` leaves room for the second halfword.
    let lim = size - 2;
    let mut p = 0usize;

    loop {
        // C: the `do { b1 = p[1]; for (;;) { ... } }` scan, which reads each
        // high byte once and tests two overlapping pairs per turn.
        let mut b1 = u32::from(data[p + 1]);
        loop {
            if p >= lim {
                return p;
            }
            let b3 = u32::from(data[p + 3]);
            p += 2;
            if b3 & (b1 ^ 8) >= 0xF8 {
                break;
            }
            if p >= lim {
                return p;
            }
            b1 = u32::from(data[p + 3]);
            p += 2;
            if b1 & (b3 ^ 8) >= 0xF8 {
                break;
            }
        }
        {
            let v = (get_u16le(data, p - 2) << 11) | (get_u16le(data, p) & 0x7FF);
            p += 2;
            let c = pc.wrapping_add(p as u32) >> 1;
            let v = convert_val(v, c, encoding);
            set_u16le(data, p - 4, (((v >> 11) & 0x7FF) | 0xF000) as u16);
            set_u16le(data, p - 2, (v | 0xF800) as u16);
        }
        if p >= lim {
            return p;
        }
    }
}

/// C: `z7_BranchConv_IA64_Dec` / `_Enc`.
fn ia64_conv(data: &mut [u8], pc: u32, encoding: bool) -> usize {
    let lim = data.len() & !15;
    let mut p = 0usize;
    // C: `pc -= 1 << 4; pc >>= 4 - 1;`
    let mut pc = pc.wrapping_sub(1 << 4) >> 3;

    loop {
        let mut m;
        loop {
            if p == lim {
                return p;
            }
            m = (0x334B_0000u32 >> (u32::from(data[p]) & 0x1E)) & 3;
            p += 16;
            pc = pc.wrapping_add(1 << 1);
            if m != 0 {
                break;
            }
        }
        // C: `p += (ptrdiff_t)m * 5 - 20`, always negative.
        p = p + m as usize * 5 - 20;
        loop {
            let t = get_u16le(data, p);
            let z = get_u32le(data, p + 1) >> m;
            p += 5;
            if (t >> m) & (0x70 << 1) == 0
                && z.wrapping_sub(0x0500_0000 << 1) & (0x0f00_0000u32 << 1) == 0
            {
                let mut v = ((0x8F_FFFFu32 << 1) | 1) & z;
                let mut z = z ^ v;
                // C: the only converter whose two directions differ by
                // more than `BR_CONVERT_VAL`. The decode arm sets the top
                // bits of `pc` rather than masking them off; both mutate the
                // local `pc` in place, and that mutation persists across
                // iterations exactly as in the C (it is idempotent either
                // way, since `pc` only ever gains `1 << 1` between turns).
                if encoding {
                    pc &= (0x1F_FFFFu32 << 1) | 1;
                    v = v.wrapping_add(pc);
                } else {
                    pc |= !((0x1F_FFFFu32 << 1) | 1);
                    v = v.wrapping_sub(pc);
                }
                v &= !(0x60_0000u32 << 1);
                v = v.wrapping_add(0x70_0000 << 1);
                v &= (0x8F_FFFFu32 << 1) | 1;
                z |= v;
                z <<= m;
                set_u32le(data, p + 1 - 5, z);
            }
            m = (m + 1) & 3;
            if m == 0 {
                break;
            }
        }
    }
}

/// C: `Z7_BRANCH_CONV_DEC(RISCV)` in `Bra.c`, xz filter id 0x0B.
///
/// RISC-V is the one converter the C writes out twice instead of taking an
/// `encoding` flag: the two directions rebuild the instruction pair from
/// different halves, and the branch that converts is the other one. See
/// [`riscv_encode`].
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn riscv_decode(data: &mut [u8], pc: u32) -> usize {
    /// C: `RISCV_CHECK_1`.
    #[inline]
    fn check1(v: u32, b: u32) -> bool {
        (b.wrapping_sub(3) ^ (v << 8)) & (0xF_8000 + 3) == 0
    }
    /// C: `RISCV_CHECK_2`.
    #[inline]
    fn check2(v: u32, r: u32) -> bool {
        (v.wrapping_sub((3 << 12) | (2 << 7) | 8) << 18) < (r & 0x1D)
    }

    let size = data.len() & !1;
    if size <= 6 {
        return 0;
    }
    let lim = size - 6;
    let mut p = 0usize;

    loop {
        // C: `RISCV_SCAN_LOOP`'s inner scan for a JAL or AUIPC opcode.
        let mut a;
        loop {
            if p >= lim {
                return p;
            }
            a = (get_u16le(data, p) ^ 0x10).wrapping_add(1);
            if a & 0x77 == 0 {
                break;
            }
            a = (get_u16le(data, p + 2) ^ 0x10).wrapping_add(1);
            p += 4;
            if a & 0x77 == 0 {
                p -= 2;
                if p >= lim {
                    return p;
                }
                break;
            }
        }

        if a & 8 == 0 {
            // JAL.
            a = a.wrapping_sub(0x100 - 0x7F);
            if a & 0xD80 != 0 {
                p += 2;
                continue;
            }
            let a_old = a.wrapping_add(0xEF - 0x7F) & 0xFFF;
            let mut v =
                (u32::from(data[p + 3]) << 1) | (u32::from(data[p + 2]) << 9) | ((a & 0xF000) << 5);
            v = v.wrapping_sub(pc.wrapping_add(p as u32));
            let a = a_old
                | (v << 11 & 1u32 << 31)
                | (v << 20 & 0x3FF << 21)
                | (v << 9 & 1 << 20)
                | (v & 0xFF << 12);
            set_u32le(data, p, a);
            p += 4;
            continue;
        }

        // AUIPC.
        let v0 = a;
        let a = get_u32le(data, p);
        if v0 & 0xE80 == 0 {
            // x0 / x2.
            let r = a >> 27;
            if check2(v0, r) {
                let mut b = get_u32be(data, p + 4);
                let mut v = a >> 12;
                b = b.wrapping_sub(pc.wrapping_add(p as u32));
                let mut w = (r << 7) + 0x17;
                w = w.wrapping_add(b.wrapping_add(0x800) & 0xFFFF_F000);
                v |= b << 20;
                set_u32le(data, p, w);
                set_u32le(data, p + 4, v);
                p += 8;
            } else {
                // C: RISCV_STEP_2.
                p += 4;
            }
        } else {
            let b = get_u32le(data, p + 4);
            if check1(v0, b) {
                let v = (a & 0xFFFF_F000) | (b >> 20);
                let w = (b << 12) | (0x17 + (2 << 7));
                set_u32le(data, p, w);
                set_u32le(data, p + 4, v);
                p += 8;
            } else {
                // C: RISCV_STEP_1.
                p += 6;
            }
        }
    }
}

/// C: `Z7_BRANCH_CONV_ENC(RISCV)` in `Bra.c`, xz filter id 0x0B.
///
/// Not a flag away from [`riscv_decode`]: the scan loop is shared
/// (`RISCV_SCAN_LOOP`), but the JAL arm rebuilds the three offset bytes from
/// the whole word rather than from the masked form, and of the two AUIPC arms
/// it is the *first* (a register other than x0/x2, `RISCV_CHECK_1`) that
/// converts, where the decoder converts in the second (`RISCV_CHECK_2`).
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn riscv_encode(data: &mut [u8], pc: u32) -> usize {
    /// C: `RISCV_CHECK_1`.
    #[inline]
    fn check1(v: u32, b: u32) -> bool {
        (b.wrapping_sub(3) ^ (v << 8)) & (0xF_8000 + 3) == 0
    }
    /// C: `RISCV_CHECK_2`.
    #[inline]
    fn check2(v: u32, r: u32) -> bool {
        (v.wrapping_sub((3 << 12) | (2 << 7) | 8) << 18) < (r & 0x1D)
    }

    let size = data.len() & !1;
    if size <= 6 {
        return 0;
    }
    let lim = size - 6;
    let mut p = 0usize;

    loop {
        // C: `RISCV_SCAN_LOOP`, byte for byte the decoder's.
        let mut a;
        loop {
            if p >= lim {
                return p;
            }
            a = (get_u16le(data, p) ^ 0x10).wrapping_add(1);
            if a & 0x77 == 0 {
                break;
            }
            a = (get_u16le(data, p + 2) ^ 0x10).wrapping_add(1);
            p += 4;
            if a & 0x77 == 0 {
                p -= 2;
                if p >= lim {
                    return p;
                }
                break;
            }
        }

        // C: `v = a; a = RISCV_GET_UI32(p);` — `v` stays the scan value and
        // `a` becomes the whole instruction word.
        let v0 = a;
        let a = get_u32le(data, p);

        if v0 & 8 == 0 {
            // JAL.
            if v0.wrapping_sub(0x100) & 0xD80 != 0 {
                p += 2;
                continue;
            }
            let mut v = ((a & (1u32 << 31)) >> 11)
                | ((a & (0x3FF << 21)) >> 20)
                | ((a & (1 << 20)) >> 9)
                | (a & (0xFF << 12));
            v = v.wrapping_add(pc.wrapping_add(p as u32));
            data[p + 1] = (((v >> 13) & 0xF0) | ((a >> 8) & 0xF)) as u8;
            data[p + 2] = (v >> 9) as u8;
            data[p + 3] = (v >> 1) as u8;
            p += 4;
            continue;
        }

        // AUIPC.
        if v0 & 0xE80 != 0 {
            // A register other than x0/x2: this is the arm that converts.
            let b = get_u32le(data, p + 4);
            if check1(v0, b) {
                let temp = (b << 12) | (0x17 + (2 << 7));
                set_u32le(data, p, temp);
                let mut w = a & 0xFFFF_F000;
                // C: the portable emulation of `(Int32)b >> 20`.
                w = w.wrapping_add((b >> 20).wrapping_sub((b >> 19) & 0x1000));
                w = w.wrapping_add(pc.wrapping_add(p as u32));
                set_u32be(data, p + 4, w);
                p += 8;
            } else {
                // C: RISCV_STEP_1.
                p += 6;
            }
        } else {
            // x0 / x2.
            let r = a >> 27;
            if check2(v0, r) {
                let v = get_u32le(data, p + 4);
                let w = (r << 7) + 0x17 + (v & 0xFFFF_F000);
                let a = (a >> 12) | (v << 20);
                set_u32le(data, p, w);
                set_u32le(data, p + 4, a);
                p += 8;
            } else {
                // C: RISCV_STEP_2.
                p += 4;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeding a buffer in pieces, carrying the tail and advancing `pc` as
    /// `Bra.h` says to, must give the same bytes as one call. This is the
    /// property the block pipeline relies on, and the one that a mishandled
    /// `pc` or state breaks.
    #[test]
    fn split_calls_match_one_call() {
        let kinds = [
            BcjKind::X86,
            BcjKind::Ppc,
            BcjKind::Ia64,
            BcjKind::Arm,
            BcjKind::ArmThumb,
            BcjKind::Sparc,
            BcjKind::Arm64,
            BcjKind::RiscV,
        ];
        // Something with plenty of accidental opcodes in it.
        let mut src = alloc::vec::Vec::new();
        let mut s = 0x1234_5678u32;
        for _ in 0..4096 {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            src.extend_from_slice(&s.to_le_bytes());
            if s & 0xF == 0 {
                src.extend_from_slice(&[0xE8, 0x00, 0x00, 0x00, 0x00]);
            }
        }

        for kind in kinds {
            let mut whole = src.clone();
            let mut one = Bcj::new(kind, 0).expect("aligned");
            let n = one.decode(&mut whole);

            for chunk in [17usize, 64, 1000, 4096] {
                let mut piecewise = alloc::vec::Vec::new();
                let mut carry: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
                let mut f = Bcj::new(kind, 0).expect("aligned");
                for part in src.chunks(chunk) {
                    carry.extend_from_slice(part);
                    let done = f.decode(&mut carry);
                    piecewise.extend_from_slice(&carry[..done]);
                    carry.drain(..done);
                }
                assert_eq!(
                    piecewise[..],
                    whole[..n.min(piecewise.len())],
                    "{kind:?} at chunk {chunk}"
                );
                assert!(
                    piecewise.len() + kind.max_carry() >= n,
                    "{kind:?} at chunk {chunk} converted far less than one call"
                );
            }
        }
    }

    #[test]
    fn a_misaligned_start_offset_is_refused() {
        assert!(Bcj::new(BcjKind::Arm64, 2).is_err());
        assert!(Bcj::new(BcjKind::Arm64, 4).is_ok());
        assert!(Bcj::new(BcjKind::X86, 3).is_ok());
        assert!(Bcj::new(BcjKind::Ia64, 8).is_err());
    }

    /// Short buffers must be refused rather than half-converted.
    #[test]
    fn a_buffer_under_the_lookahead_converts_nothing() {
        for kind in [BcjKind::X86, BcjKind::ArmThumb, BcjKind::RiscV] {
            let mut buf = [0xE8u8; 4];
            let mut f = Bcj::new(kind, 0).expect("aligned");
            assert_eq!(f.decode(&mut buf[..2]), 0, "{kind:?}");
        }
    }

    #[test]
    fn encoding_then_decoding_gives_the_input_back() {
        // A converter is only an involution up to its own lookahead: whatever
        // it could not convert at the end of the buffer is left alone by both
        // directions, so the pair is the identity over the whole buffer.
        let kinds = [
            BcjKind::X86,
            BcjKind::Ppc,
            BcjKind::Ia64,
            BcjKind::Arm,
            BcjKind::ArmThumb,
            BcjKind::Sparc,
            BcjKind::Arm64,
            BcjKind::RiscV,
        ];
        for kind in kinds {
            for len in [0usize, 1, 5, 17, 64, 1000, 4099] {
                let src: Vec<u8> = (0..len)
                    .map(|i| {
                        let x = (i as u32).wrapping_mul(2_654_435_761);
                        if i % 9 == 0 { 0xE8 } else { (x >> 13) as u8 }
                    })
                    .collect();
                let mut buf = src.clone();
                let start = kind.alignment() * 2;
                Bcj::new(kind, start).unwrap().encode(&mut buf);
                Bcj::new(kind, start).unwrap().decode(&mut buf);
                assert_eq!(buf, src, "{kind:?}, {len} bytes");
            }
        }
    }
}
