//! Constants and probability-table offsets.
//!
//! C: the `#define` block at the top of `C/LzmaDec.c`. Names are the C names
//! in `SCREAMING_SNAKE_CASE`.

/// C: `kTopValue`.
pub(crate) const K_TOP_VALUE: u32 = 1 << 24;

/// C: `kNumBitModelTotalBits`.
pub(crate) const K_NUM_BIT_MODEL_TOTAL_BITS: u32 = 11;
/// C: `kBitModelTotal`.
pub(crate) const K_BIT_MODEL_TOTAL: u32 = 1 << K_NUM_BIT_MODEL_TOTAL_BITS;
/// C: `kNumMoveBits`.
pub(crate) const K_NUM_MOVE_BITS: u32 = 5;

/// C: `RC_INIT_SIZE`.
pub(crate) const RC_INIT_SIZE: usize = 5;

/// C: `LZMA_REQUIRED_INPUT_MAX` (`C/LzmaDec.h`). Number of input bytes needed
/// for the worst-case single LZMA symbol; the margin the fast loop relies on.
pub const LZMA_REQUIRED_INPUT_MAX: usize = 20;

/// C: `LZMA_PROPS_SIZE` (`C/LzmaDec.h`).
pub const LZMA_PROPS_SIZE: usize = 5;

/// C: `kNumPosBitsMax`.
pub(crate) const K_NUM_POS_BITS_MAX: u32 = 4;
/// C: `kNumPosStatesMax`.
pub(crate) const K_NUM_POS_STATES_MAX: usize = 1 << K_NUM_POS_BITS_MAX;

/// C: `kLenNumLowBits`.
pub(crate) const K_LEN_NUM_LOW_BITS: u32 = 3;
/// C: `kLenNumLowSymbols`.
pub(crate) const K_LEN_NUM_LOW_SYMBOLS: u32 = 1 << K_LEN_NUM_LOW_BITS;
/// C: `kLenNumHighBits`.
pub(crate) const K_LEN_NUM_HIGH_BITS: u32 = 8;
/// C: `kLenNumHighSymbols`.
pub(crate) const K_LEN_NUM_HIGH_SYMBOLS: u32 = 1 << K_LEN_NUM_HIGH_BITS;

/// C: `LenLow`. Offset within a length coder's sub-table.
pub(crate) const LEN_LOW: usize = 0;
/// C: `LenHigh`.
pub(crate) const LEN_HIGH: usize = LEN_LOW + 2 * (K_NUM_POS_STATES_MAX << K_LEN_NUM_LOW_BITS);
/// C: `kNumLenProbs`.
pub(crate) const K_NUM_LEN_PROBS: usize = LEN_HIGH + K_LEN_NUM_HIGH_SYMBOLS as usize;

/// C: `LenChoice`.
pub(crate) const LEN_CHOICE: usize = LEN_LOW;
/// C: `LenChoice2`.
pub(crate) const LEN_CHOICE_2: usize = LEN_LOW + (1 << K_LEN_NUM_LOW_BITS);

/// C: `kNumStates`.
pub(crate) const K_NUM_STATES: u32 = 12;
/// C: `kNumStates2`.
pub(crate) const K_NUM_STATES2: usize = 16;
/// C: `kNumLitStates`.
pub(crate) const K_NUM_LIT_STATES: u32 = 7;

/// C: `kStartPosModelIndex`.
pub(crate) const K_START_POS_MODEL_INDEX: u32 = 4;
/// C: `kEndPosModelIndex`.
pub(crate) const K_END_POS_MODEL_INDEX: u32 = 14;
/// C: `kNumFullDistances`.
pub(crate) const K_NUM_FULL_DISTANCES: usize = 1 << (K_END_POS_MODEL_INDEX >> 1);

/// C: `kNumPosSlotBits`.
pub(crate) const K_NUM_POS_SLOT_BITS: u32 = 6;
/// C: `kNumLenToPosStates`.
pub(crate) const K_NUM_LEN_TO_POS_STATES: usize = 4;

/// C: `kNumAlignBits`.
pub(crate) const K_NUM_ALIGN_BITS: u32 = 4;
/// C: `kAlignTableSize`.
pub(crate) const K_ALIGN_TABLE_SIZE: usize = 1 << K_NUM_ALIGN_BITS;

/// C: `kMatchMinLen`.
pub(crate) const K_MATCH_MIN_LEN: u32 = 2;
/// C: `kMatchSpecLenStart`.
pub(crate) const K_MATCH_SPEC_LEN_START: u32 =
    K_MATCH_MIN_LEN + K_LEN_NUM_LOW_SYMBOLS * 2 + K_LEN_NUM_HIGH_SYMBOLS;

/// C: `kMatchSpecLen_Error_Data`.
pub(crate) const K_MATCH_SPEC_LEN_ERROR_DATA: u32 = 1 << 9;
/// C: `kMatchSpecLen_Error_Fail`.
pub(crate) const K_MATCH_SPEC_LEN_ERROR_FAIL: u32 = K_MATCH_SPEC_LEN_ERROR_DATA - 1;

// ---------------------------------------------------------------------------
// Probability table layout.
//
// C: the offsets below are written relative to `p->probs_1664`, i.e. with
// `kStartOffset = 1664` subtracted, so `SpecPos` is negative there. The port
// keeps one flat `probs` slice and uses the same offsets with `kStartOffset`
// added back, which makes every index non-negative and identical to the
// absolute index the C code addresses. The layout itself is unchanged; the
// external ASM loops depend on it and so do the tests.
// ---------------------------------------------------------------------------

/// C: `kStartOffset`.
pub(crate) const K_START_OFFSET: usize = 1664;

/// C: `SpecPos` (`-kStartOffset`), absolute.
pub(crate) const SPEC_POS: usize = 0;
/// C: `IsRep0Long`, absolute.
pub(crate) const IS_REP0_LONG: usize = SPEC_POS + K_NUM_FULL_DISTANCES;
/// C: `RepLenCoder`, absolute.
pub(crate) const REP_LEN_CODER: usize = IS_REP0_LONG + (K_NUM_STATES2 << K_NUM_POS_BITS_MAX);
/// C: `LenCoder`, absolute.
pub(crate) const LEN_CODER: usize = REP_LEN_CODER + K_NUM_LEN_PROBS;
/// C: `IsMatch`, absolute.
pub(crate) const IS_MATCH: usize = LEN_CODER + K_NUM_LEN_PROBS;
/// C: `Align`, absolute. Equals `kStartOffset` exactly, as the C asserts.
pub(crate) const ALIGN: usize = IS_MATCH + (K_NUM_STATES2 << K_NUM_POS_BITS_MAX);
/// C: `IsRep`, absolute.
pub(crate) const IS_REP: usize = ALIGN + K_ALIGN_TABLE_SIZE;
/// C: `IsRepG0`, absolute.
pub(crate) const IS_REP_G0: usize = IS_REP + K_NUM_STATES as usize;
/// C: `IsRepG1`, absolute.
pub(crate) const IS_REP_G1: usize = IS_REP_G0 + K_NUM_STATES as usize;
/// C: `IsRepG2`, absolute.
pub(crate) const IS_REP_G2: usize = IS_REP_G1 + K_NUM_STATES as usize;
/// C: `PosSlot`, absolute.
pub(crate) const POS_SLOT: usize = IS_REP_G2 + K_NUM_STATES as usize;
/// C: `Literal`, absolute.
pub(crate) const LITERAL: usize = POS_SLOT + (K_NUM_LEN_TO_POS_STATES << K_NUM_POS_SLOT_BITS);
/// C: `NUM_BASE_PROBS`.
pub(crate) const NUM_BASE_PROBS: usize = LITERAL;

// C: `#if Align != 0 && kStartOffset != 0  #error Stop_Compiling_Bad_LZMA_kAlign`
const _: () = assert!(ALIGN == K_START_OFFSET);
// C: `#if NUM_BASE_PROBS != 1984  #error Stop_Compiling_Bad_LZMA_PROBS`
const _: () = assert!(NUM_BASE_PROBS == 1984);

/// C: `LZMA_LIT_SIZE`.
pub(crate) const LZMA_LIT_SIZE: usize = 0x300;

/// C: `LZMA_DIC_MIN`.
pub(crate) const LZMA_DIC_MIN: u32 = 1 << 12;

/// C: `kBadRepCode`. The range coder `code` value above which the very first
/// symbol of a stream could only be a rep match, which is illegal; checked
/// early so the fast loop need not test for it.
pub(crate) const K_BAD_REP_CODE: u32 = 0xC000_0000 - 0x400;

// C: `#if kBadRepCode != (0xC0000000 - 0x400) #error Stop_Compiling_Bad_LZMA_Check`
const _: () = {
    const K_RANGE0: u32 = 0xFFFF_FFFF;
    const K_BOUND0: u32 =
        (K_RANGE0 >> K_NUM_BIT_MODEL_TOTAL_BITS) << (K_NUM_BIT_MODEL_TOTAL_BITS - 1);
    assert!(
        K_BAD_REP_CODE
            == K_BOUND0
                + (((K_RANGE0 - K_BOUND0) >> K_NUM_BIT_MODEL_TOTAL_BITS)
                    << (K_NUM_BIT_MODEL_TOTAL_BITS - 1))
    );
};

/// C: `LzmaProps_GetNumProbs(p)`.
#[inline]
pub(crate) const fn lzma_props_get_num_probs(lc: u8, lp: u8) -> usize {
    NUM_BASE_PROBS + (LZMA_LIT_SIZE << (lc + lp))
}
