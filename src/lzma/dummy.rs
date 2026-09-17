//! The careful path used near buffer edges.
//!
//! C: `LzmaDec_TryDummy` in `C/LzmaDec.c`. It decodes one symbol without
//! touching decoder state, purely to learn how many input bytes that symbol
//! needs and what kind of symbol it is. Everything here is checked Rust: the
//! whole point of the function is that the margin invariant does *not* hold.

use crate::lzma::consts::*;
use crate::lzma::state::LzmaDec;

/// C: `ELzmaDummy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dummy {
    /// C: `DUMMY_INPUT_EOF`. Needs more input data.
    InputEof,
    /// C: `DUMMY_LIT`.
    Lit,
    /// C: `DUMMY_MATCH`.
    Match,
    /// C: `DUMMY_REP`.
    Rep,
}

impl Dummy {
    /// C: `IS_DUMMY_END_MARKER_POSSIBLE(dummyRes)`.
    pub(crate) fn end_marker_possible(self) -> bool {
        self == Dummy::Match
    }
}

/// C: `NORMALIZE_CHECK`.
macro_rules! normalize_check {
    ($range:ident, $code:ident, $buf:ident, $pos:ident) => {
        if $range < K_TOP_VALUE {
            if $pos >= $buf.len() {
                return (Dummy::InputEof, $pos);
            }
            $range <<= 8;
            $code = ($code << 8) | u32::from($buf[$pos]);
            $pos += 1;
        }
    };
}

/// C: `UPDATE_0_CHECK`.
macro_rules! update_0_check {
    ($range:ident, $bound:ident) => {
        $range = $bound;
    };
}

/// C: `UPDATE_1_CHECK`.
macro_rules! update_1_check {
    ($range:ident, $code:ident, $bound:ident) => {{
        $range -= $bound;
        $code -= $bound;
    }};
}

/// C: `GET_BIT2_CHECK(p, i, A0, A1)` with both actions empty, i.e.
/// `GET_BIT_CHECK`.
macro_rules! get_bit_check {
    ($probs:expr, $idx:expr, $i:ident, $ttt:ident, $bound:ident,
     $range:ident, $code:ident, $buf:ident, $pos:ident) => {{
        $ttt = u32::from($probs[$idx]);
        normalize_check!($range, $code, $buf, $pos);
        $bound = ($range >> K_NUM_BIT_MODEL_TOTAL_BITS) * $ttt;
        if $code < $bound {
            update_0_check!($range, $bound);
            $i = $i + $i;
        } else {
            update_1_check!($range, $code, $bound);
            $i = $i + $i + 1;
        }
    }};
}

/// C: `LzmaDec_TryDummy`.
///
/// `buf` is the input available from the current position (the C's
/// `buf .. *bufOut`). Returns the symbol kind and how many bytes of `buf` it
/// consumed; on [`Dummy::InputEof`] the byte count is meaningless, exactly as
/// the C caller treats it.
#[allow(clippy::too_many_lines)]
// The port keeps the reference decoder's dead stores (the last `NORMALIZE`,
// the last `offs ^= bit`) so the expansion matches the C line for line.
#[allow(unused_assignments)]
pub(crate) fn lzma_dec_try_dummy(p: &LzmaDec, buf: &[u8]) -> (Dummy, usize) {
    let mut range: u32 = p.range;
    let mut code: u32 = p.code;
    let probs: &[u16] = &p.probs;
    let mut state: u32 = p.state;
    let res: Dummy;
    let mut pos: usize = 0;

    let mut bound: u32;
    let mut ttt: u32;

    'once: {
        let pos_state: u32 = (p.processed_pos & ((1u32 << p.prop.pb()) - 1)) << 4;

        let mut prob_base: usize = IS_MATCH + (pos_state + state) as usize;
        ttt = u32::from(probs[prob_base]);
        normalize_check!(range, code, buf, pos);
        bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
        if code < bound {
            update_0_check!(range, bound);

            prob_base = LITERAL;
            if p.check_dic_size != 0 || p.processed_pos != 0 {
                let prev = u32::from(
                    p.dic[(if p.dic_pos == 0 {
                        p.dic_buf_size
                    } else {
                        p.dic_pos
                    }) - 1],
                );
                prob_base += LZMA_LIT_SIZE
                    * (((p.processed_pos & ((1u32 << p.prop.lp()) - 1)) << p.prop.lc())
                        + (prev >> (8 - u32::from(p.prop.lc())))) as usize;
            }

            if state < K_NUM_LIT_STATES {
                let mut symbol: u32 = 1;
                loop {
                    get_bit_check!(
                        probs,
                        prob_base + symbol as usize,
                        symbol,
                        ttt,
                        bound,
                        range,
                        code,
                        buf,
                        pos
                    );
                    if symbol >= 0x100 {
                        break;
                    }
                }
            } else {
                let mut match_byte = u32::from(
                    p.dic[p.dic_pos.wrapping_sub(p.reps[0] as usize).wrapping_add(
                        if p.dic_pos < p.reps[0] as usize {
                            p.dic_buf_size
                        } else {
                            0
                        },
                    )],
                );
                let mut offs: u32 = 0x100;
                let mut symbol: u32 = 1;
                loop {
                    match_byte += match_byte;
                    let bit = offs;
                    offs &= match_byte;
                    let prob_lit = prob_base + (offs + bit + symbol) as usize;
                    ttt = u32::from(probs[prob_lit]);
                    normalize_check!(range, code, buf, pos);
                    bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
                    if code < bound {
                        update_0_check!(range, bound);
                        symbol = symbol + symbol;
                        offs ^= bit;
                    } else {
                        update_1_check!(range, code, bound);
                        symbol = symbol + symbol + 1;
                    }
                    if symbol >= 0x100 {
                        break;
                    }
                }
            }
            res = Dummy::Lit;
        } else {
            let mut len: u32;
            update_1_check!(range, code, bound);

            prob_base = IS_REP + state as usize;
            ttt = u32::from(probs[prob_base]);
            normalize_check!(range, code, buf, pos);
            bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
            if code < bound {
                update_0_check!(range, bound);
                state = 0;
                prob_base = LEN_CODER;
                res = Dummy::Match;
            } else {
                update_1_check!(range, code, bound);
                res = Dummy::Rep;
                prob_base = IS_REP_G0 + state as usize;
                ttt = u32::from(probs[prob_base]);
                normalize_check!(range, code, buf, pos);
                bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
                if code < bound {
                    update_0_check!(range, bound);
                    prob_base = IS_REP0_LONG + (pos_state + state) as usize;
                    ttt = u32::from(probs[prob_base]);
                    normalize_check!(range, code, buf, pos);
                    bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
                    if code < bound {
                        update_0_check!(range, bound);
                        break 'once;
                    }
                    update_1_check!(range, code, bound);
                } else {
                    update_1_check!(range, code, bound);
                    prob_base = IS_REP_G1 + state as usize;
                    ttt = u32::from(probs[prob_base]);
                    normalize_check!(range, code, buf, pos);
                    bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
                    if code < bound {
                        update_0_check!(range, bound);
                    } else {
                        update_1_check!(range, code, bound);
                        prob_base = IS_REP_G2 + state as usize;
                        ttt = u32::from(probs[prob_base]);
                        normalize_check!(range, code, buf, pos);
                        bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
                        if code < bound {
                            update_0_check!(range, bound);
                        } else {
                            update_1_check!(range, code, bound);
                        }
                    }
                }
                state = K_NUM_STATES;
                prob_base = REP_LEN_CODER;
            }
            {
                let limit: u32;
                let offset: u32;
                let mut prob_len = prob_base + LEN_CHOICE;
                ttt = u32::from(probs[prob_len]);
                normalize_check!(range, code, buf, pos);
                bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
                if code < bound {
                    update_0_check!(range, bound);
                    prob_len = prob_base + LEN_LOW + pos_state as usize;
                    offset = 0;
                    limit = 1 << K_LEN_NUM_LOW_BITS;
                } else {
                    update_1_check!(range, code, bound);
                    prob_len = prob_base + LEN_CHOICE_2;
                    ttt = u32::from(probs[prob_len]);
                    normalize_check!(range, code, buf, pos);
                    bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
                    if code < bound {
                        update_0_check!(range, bound);
                        prob_len =
                            prob_base + LEN_LOW + pos_state as usize + (1 << K_LEN_NUM_LOW_BITS);
                        offset = K_LEN_NUM_LOW_SYMBOLS;
                        limit = 1 << K_LEN_NUM_LOW_BITS;
                    } else {
                        update_1_check!(range, code, bound);
                        prob_len = prob_base + LEN_HIGH;
                        offset = K_LEN_NUM_LOW_SYMBOLS * 2;
                        limit = 1 << K_LEN_NUM_HIGH_BITS;
                    }
                }
                // C: TREE_DECODE_CHECK(probLen, limit, len)
                len = 1;
                loop {
                    get_bit_check!(
                        probs,
                        prob_len + len as usize,
                        len,
                        ttt,
                        bound,
                        range,
                        code,
                        buf,
                        pos
                    );
                    if len >= limit {
                        break;
                    }
                }
                len -= limit;
                len += offset;
            }

            if state < 4 {
                {
                    let temp = (if len < K_NUM_LEN_TO_POS_STATES as u32 - 1 {
                        len
                    } else {
                        K_NUM_LEN_TO_POS_STATES as u32 - 1
                    }) << K_NUM_POS_SLOT_BITS;
                    prob_base = POS_SLOT + temp as usize;
                }
                // C: TREE_DECODE_CHECK(prob, 1 << kNumPosSlotBits, posSlot)
                let mut i: u32 = 1;
                loop {
                    get_bit_check!(
                        probs,
                        prob_base + i as usize,
                        i,
                        ttt,
                        bound,
                        range,
                        code,
                        buf,
                        pos
                    );
                    if i >= (1 << K_NUM_POS_SLOT_BITS) {
                        break;
                    }
                }
                let pos_slot = i - (1 << K_NUM_POS_SLOT_BITS);

                if pos_slot >= K_START_POS_MODEL_INDEX {
                    let mut num_direct_bits = (pos_slot >> 1) - 1;

                    if pos_slot < K_END_POS_MODEL_INDEX {
                        let temp = (2 | (pos_slot & 1)) << num_direct_bits;
                        prob_base = SPEC_POS + temp as usize;
                    } else {
                        num_direct_bits -= K_NUM_ALIGN_BITS;
                        loop {
                            normalize_check!(range, code, buf, pos);
                            range >>= 1;
                            code -= range & (((code.wrapping_sub(range)) >> 31).wrapping_sub(1));
                            /* if (code >= range) code -= range; */
                            num_direct_bits -= 1;
                            if num_direct_bits == 0 {
                                break;
                            }
                        }
                        prob_base = ALIGN;
                        num_direct_bits = K_NUM_ALIGN_BITS;
                    }
                    {
                        let mut i: u32 = 1;
                        let mut m: u32 = 1;
                        loop {
                            // C: REV_BIT_CHECK(prob, i, m)
                            ttt = u32::from(probs[prob_base + i as usize]);
                            normalize_check!(range, code, buf, pos);
                            bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
                            if code < bound {
                                update_0_check!(range, bound);
                                i += m;
                                m += m;
                            } else {
                                update_1_check!(range, code, bound);
                                m += m;
                                i += m;
                            }
                            num_direct_bits -= 1;
                            if num_direct_bits == 0 {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    normalize_check!(range, code, buf, pos);

    (res, pos)
}
