//! The LZMA fast decode loop.
//!
//! C: `LzmaDec_DecodeReal_3` (`LZMA_DECODE_REAL`) in `C/LzmaDec.c`, the
//! `#ifndef Z7_LZMA_DEC_OPT` C path. The hand-written assembly variants in
//! `Asm/x86/LzmaDecOpt.asm` and `Asm/arm64/LzmaDecOpt.S` are out of scope.

use crate::lzma::consts::*;
use crate::lzma::state::LzmaDec;

// ---------------------------------------------------------------------------
// Range coder macros.
//
// C: `NORMALIZE`, `IF_BIT_0`, `UPDATE_0`, `UPDATE_1`, `GET_BIT2`,
// `TREE_GET_BIT`, `REV_BIT*`, `MATCHED_LITER_DEC`. Rust's `macro_rules!` is
// hygienic, so every local the C macro touches is threaded through as a
// parameter; the expansion is otherwise identical.
// ---------------------------------------------------------------------------

/// C: `NORMALIZE`.
macro_rules! normalize {
    ($range:ident, $code:ident, $buf:ident) => {
        if $range < K_TOP_VALUE {
            $range <<= 8;
            // SAFETY: the caller guarantees the margin invariant (see the
            // SAFETY block in `lzma_dec_decode_real`): `buf` never advances
            // past `buf_limit + LZMA_REQUIRED_INPUT_MAX`, and the caller sized
            // the input so that range is readable.
            $code = ($code << 8) | (*$buf as u32);
            $buf = $buf.add(1);
        }
    };
}

/// C: `UPDATE_0(p)`.
macro_rules! update_0 {
    ($prob:expr, $range:ident, $bound:ident, $ttt:ident) => {{
        $range = $bound;
        *$prob = ($ttt + ((K_BIT_MODEL_TOTAL - $ttt) >> K_NUM_MOVE_BITS)) as u16;
    }};
}

/// C: `UPDATE_1(p)`.
macro_rules! update_1 {
    ($prob:expr, $range:ident, $code:ident, $bound:ident, $ttt:ident) => {{
        $range -= $bound;
        $code -= $bound;
        *$prob = ($ttt - ($ttt >> K_NUM_MOVE_BITS)) as u16;
    }};
}

/// C: `TREE_GET_BIT(probs, i)`, i.e. `GET_BIT2(probs + i, i, ;, ;)`.
macro_rules! tree_get_bit {
    ($probs:expr, $i:ident, $ttt:ident, $bound:ident, $range:ident, $code:ident, $buf:ident) => {{
        let prob_ = $probs.add($i as usize);
        $ttt = *prob_ as u32;
        normalize!($range, $code, $buf);
        $bound = ($range >> K_NUM_BIT_MODEL_TOTAL_BITS) * $ttt;
        if $code < $bound {
            update_0!(prob_, $range, $bound, $ttt);
            $i = $i + $i;
        } else {
            update_1!(prob_, $range, $code, $bound, $ttt);
            $i = $i + $i + 1;
        }
    }};
}

/// C: `MATCHED_LITER_DEC`.
macro_rules! matched_liter_dec {
    ($prob:expr, $symbol:ident, $match_byte:ident, $offs:ident,
     $ttt:ident, $bound:ident, $range:ident, $code:ident, $buf:ident) => {{
        $match_byte += $match_byte;
        let bit = $offs;
        $offs &= $match_byte;
        let prob_lit = $prob.add(($offs + bit + $symbol) as usize);
        $ttt = *prob_lit as u32;
        normalize!($range, $code, $buf);
        $bound = ($range >> K_NUM_BIT_MODEL_TOTAL_BITS) * $ttt;
        if $code < $bound {
            update_0!(prob_lit, $range, $bound, $ttt);
            $symbol = $symbol + $symbol;
            $offs ^= bit;
        } else {
            update_1!(prob_lit, $range, $code, $bound, $ttt);
            $symbol = $symbol + $symbol + 1;
        }
    }};
}

/// C: `REV_BIT_VAR(p, i, m)`.
macro_rules! rev_bit_var {
    ($probs:expr, $i:ident, $m:ident, $ttt:ident, $bound:ident,
     $range:ident, $code:ident, $buf:ident) => {{
        let prob_ = $probs.add($i as usize);
        $ttt = *prob_ as u32;
        normalize!($range, $code, $buf);
        $bound = ($range >> K_NUM_BIT_MODEL_TOTAL_BITS) * $ttt;
        if $code < $bound {
            update_0!(prob_, $range, $bound, $ttt);
            $i += $m;
            $m += $m;
        } else {
            update_1!(prob_, $range, $code, $bound, $ttt);
            $m += $m;
            $i += $m;
        }
    }};
}

/// C: `REV_BIT_CONST(p, i, m)`.
macro_rules! rev_bit_const {
    ($probs:expr, $i:ident, $m:expr, $ttt:ident, $bound:ident,
     $range:ident, $code:ident, $buf:ident) => {{
        let prob_ = $probs.add($i as usize);
        $ttt = *prob_ as u32;
        normalize!($range, $code, $buf);
        $bound = ($range >> K_NUM_BIT_MODEL_TOTAL_BITS) * $ttt;
        if $code < $bound {
            update_0!(prob_, $range, $bound, $ttt);
            $i += $m;
        } else {
            update_1!(prob_, $range, $code, $bound, $ttt);
            $i += $m * 2;
        }
    }};
}

/// C: `REV_BIT_LAST(p, i, m)`.
macro_rules! rev_bit_last {
    ($probs:expr, $i:ident, $m:expr, $ttt:ident, $bound:ident,
     $range:ident, $code:ident, $buf:ident) => {{
        let prob_ = $probs.add($i as usize);
        $ttt = *prob_ as u32;
        normalize!($range, $code, $buf);
        $bound = ($range >> K_NUM_BIT_MODEL_TOTAL_BITS) * $ttt;
        if $code < $bound {
            update_0!(prob_, $range, $bound, $ttt);
            $i -= $m;
        } else {
            update_1!(prob_, $range, $code, $bound, $ttt);
        }
    }};
}

/// Result of one fast-loop call: `Ok` mirrors `SZ_OK`, `Err` mirrors
/// `SZ_ERROR_DATA`. The advanced input pointer is returned either way, as the
/// C leaves it in `p->buf`.
pub(crate) struct RealResult {
    pub(crate) buf: *const u8,
    pub(crate) ok: bool,
}

/// C: `LzmaDec_DecodeReal_3`.
///
/// In:
/// - the range coder is normalized;
/// - if `p.dic_pos == limit`, `lzma_dec_try_dummy` was called before to
///   exclude the LITERAL and MATCH-REP cases, so the first symbol can only be
///   a MATCH-NON-REP;
/// - `buf_start .. buf_limit + LZMA_REQUIRED_INPUT_MAX` is readable.
///
/// Processing: the first LZMA symbol is decoded in any case. All main limit
/// checks are at the end of the main loop; it decodes additional symbols while
/// `buf < buf_limit && dic_pos < limit`. The range coder is still without its
/// last normalization when `buf < buf_limit` is checked, but if `buf <
/// buf_limit` the caller provided at least `LZMA_REQUIRED_INPUT_MAX + 1` bytes
/// before `buf_limit + LZMA_REQUIRED_INPUT_MAX`, which is enough for the worst
/// case LZMA symbol plus one additional normalization for one bit. So the
/// function never reads the `buf_limit[LZMA_REQUIRED_INPUT_MAX]` byte.
///
/// Out: the range coder is normalized. `ok == false` corresponds to
/// `SZ_ERROR_DATA`, when a match symbol refers outside the dictionary.
///
/// # Safety
///
/// The caller must uphold the contract above: `limit <= p.dic_buf_size`,
/// `p.dic_pos <= limit`, `buf_start <= buf_limit`, and at least
/// `LZMA_REQUIRED_INPUT_MAX` readable bytes past `buf_limit`.
#[allow(clippy::too_many_lines)]
// The port keeps the reference decoder's dead stores (the last `NORMALIZE`,
// the last `offs ^= bit`) so the expansion matches the C line for line.
#[allow(unused_assignments)]
pub(crate) unsafe fn lzma_dec_decode_real(
    p: &mut LzmaDec,
    limit: usize,
    buf_start: *const u8,
    buf_limit: *const u8,
) -> RealResult {
    let probs: *mut u16 = p.probs.as_mut_ptr();
    let mut state: u32 = p.state;
    let mut rep0: u32 = p.reps[0];
    let mut rep1: u32 = p.reps[1];
    let mut rep2: u32 = p.reps[2];
    let mut rep3: u32 = p.reps[3];
    let pb_mask: u32 = (1u32 << p.prop.pb()) - 1;
    let lc: u32 = u32::from(p.prop.lc());
    let lp_mask: u32 = (0x100u32 << p.prop.lp()) - (0x100u32 >> lc);

    let dic: *mut u8 = p.dic.as_mut_ptr();
    let dic_buf_size: usize = p.dic_buf_size;
    let mut dic_pos: usize = p.dic_pos;

    let mut processed_pos: u32 = p.processed_pos;
    let check_dic_size: u32 = p.check_dic_size;
    let mut len: u32 = 0;

    let mut buf: *const u8 = buf_start;
    let mut range: u32 = p.range;
    let mut code: u32 = p.code;

    // SAFETY: this block is the port of the C fast loop and is unchecked in
    // exactly the places the C is. Three invariants make it sound, all
    // established by the caller `lzma_dec_decode_to_dic` before the call:
    //
    // 1. Input margin. `buf_limit + LZMA_REQUIRED_INPUT_MAX` is within the
    //    caller's input buffer (`bufLimit = src + inSize - 20` in the C, or
    //    `bufLimit = src` for the single-symbol tempBuf path, where the temp
    //    buffer is 20 bytes). The loop only continues while `buf < buf_limit`,
    //    and one iteration consumes at most `LZMA_REQUIRED_INPUT_MAX` bytes,
    //    so `*buf` in `normalize!` is always in bounds.
    // 2. Probability indices. `probs` holds `lzma_props_get_num_probs(lc, lp)`
    //    entries; every offset used below is one of the `consts` layout
    //    offsets plus a tree index bounded by that sub-table's size, and the
    //    literal sub-table index is masked with `lp_mask` and shifted by `lc`,
    //    which is exactly the range the allocation was sized for.
    // 3. Dictionary indices. `dic_pos < limit <= p.dic_buf_size` holds on
    //    entry and is re-checked at the bottom of the loop, and every read
    //    index is `dic_pos - rep0` folded into `[0, dic_buf_size)` by adding
    //    `dic_buf_size` when `dic_pos < rep0`. `rep0` is checked against
    //    `check_dic_size`/`processed_pos` before any copy uses it, which is
    //    the check that keeps distances inside the written part of the
    //    dictionary.
    unsafe {
        'outer: loop {
            'body: {
                let mut bound: u32;
                let mut ttt: u32;
                // C: CALC_POS_STATE(processedPos, pbMask)
                let pos_state: u32 = (processed_pos & pb_mask) << 4;

                // C: COMBINED_PS_STATE == posState + state
                let prob = probs.add(IS_MATCH + (pos_state + state) as usize);
                ttt = *prob as u32;
                normalize!(range, code, buf);
                bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
                if code < bound {
                    let mut symbol: u32;
                    update_0!(prob, range, bound, ttt);
                    let mut prob = probs.add(LITERAL);
                    if processed_pos != 0 || check_dic_size != 0 {
                        let prev = u32::from(
                            *dic.add(if dic_pos == 0 { dic_buf_size } else { dic_pos } - 1),
                        );
                        prob = prob.add(
                            (3 * ((((processed_pos << 8).wrapping_add(prev)) & lp_mask) << lc))
                                as usize,
                        );
                    }
                    processed_pos = processed_pos.wrapping_add(1);

                    if state < K_NUM_LIT_STATES {
                        state -= if state < 4 { state } else { 3 };
                        symbol = 1;
                        // C: NORMAL_LITER_DEC x8
                        tree_get_bit!(prob, symbol, ttt, bound, range, code, buf);
                        tree_get_bit!(prob, symbol, ttt, bound, range, code, buf);
                        tree_get_bit!(prob, symbol, ttt, bound, range, code, buf);
                        tree_get_bit!(prob, symbol, ttt, bound, range, code, buf);
                        tree_get_bit!(prob, symbol, ttt, bound, range, code, buf);
                        tree_get_bit!(prob, symbol, ttt, bound, range, code, buf);
                        tree_get_bit!(prob, symbol, ttt, bound, range, code, buf);
                        tree_get_bit!(prob, symbol, ttt, bound, range, code, buf);
                    } else {
                        let mut match_byte =
                            u32::from(*dic.add(dic_pos.wrapping_sub(rep0 as usize).wrapping_add(
                                if dic_pos < rep0 as usize {
                                    dic_buf_size
                                } else {
                                    0
                                },
                            )));
                        let mut offs: u32 = 0x100;
                        state -= if state < 10 { 3 } else { 6 };
                        symbol = 1;
                        // C: MATCHED_LITER_DEC x8
                        matched_liter_dec!(
                            prob, symbol, match_byte, offs, ttt, bound, range, code, buf
                        );
                        matched_liter_dec!(
                            prob, symbol, match_byte, offs, ttt, bound, range, code, buf
                        );
                        matched_liter_dec!(
                            prob, symbol, match_byte, offs, ttt, bound, range, code, buf
                        );
                        matched_liter_dec!(
                            prob, symbol, match_byte, offs, ttt, bound, range, code, buf
                        );
                        matched_liter_dec!(
                            prob, symbol, match_byte, offs, ttt, bound, range, code, buf
                        );
                        matched_liter_dec!(
                            prob, symbol, match_byte, offs, ttt, bound, range, code, buf
                        );
                        matched_liter_dec!(
                            prob, symbol, match_byte, offs, ttt, bound, range, code, buf
                        );
                        matched_liter_dec!(
                            prob, symbol, match_byte, offs, ttt, bound, range, code, buf
                        );
                    }

                    *dic.add(dic_pos) = symbol as u8;
                    dic_pos += 1;
                    break 'body; // C: continue
                }

                {
                    update_1!(prob, range, code, bound, ttt);
                    let mut prob = probs.add(IS_REP + state as usize);
                    ttt = *prob as u32;
                    normalize!(range, code, buf);
                    bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
                    if code < bound {
                        update_0!(prob, range, bound, ttt);
                        state += K_NUM_STATES;
                        prob = probs.add(LEN_CODER);
                    } else {
                        update_1!(prob, range, code, bound, ttt);
                        prob = probs.add(IS_REP_G0 + state as usize);
                        ttt = *prob as u32;
                        normalize!(range, code, buf);
                        bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
                        if code < bound {
                            update_0!(prob, range, bound, ttt);
                            prob = probs.add(IS_REP0_LONG + (pos_state + state) as usize);
                            ttt = *prob as u32;
                            normalize!(range, code, buf);
                            bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
                            if code < bound {
                                update_0!(prob, range, bound, ttt);

                                // that case was checked before with kBadRepCode
                                // if (checkDicSize == 0 && processedPos == 0) { len = kMatchSpecLen_Error_Data + 1; break; }
                                // The caller doesn't allow (dicPos == limit) case here
                                // so we don't need the following check:
                                // if (dicPos == limit) { state = state < kNumLitStates ? 9 : 11; len = 1; break; }

                                *dic.add(dic_pos) =
                                    *dic.add(dic_pos.wrapping_sub(rep0 as usize).wrapping_add(
                                        if dic_pos < rep0 as usize {
                                            dic_buf_size
                                        } else {
                                            0
                                        },
                                    ));
                                dic_pos += 1;
                                processed_pos = processed_pos.wrapping_add(1);
                                state = if state < K_NUM_LIT_STATES { 9 } else { 11 };
                                break 'body; // C: continue
                            }
                            update_1!(prob, range, code, bound, ttt);
                        } else {
                            let distance: u32;
                            update_1!(prob, range, code, bound, ttt);
                            prob = probs.add(IS_REP_G1 + state as usize);
                            ttt = *prob as u32;
                            normalize!(range, code, buf);
                            bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
                            if code < bound {
                                update_0!(prob, range, bound, ttt);
                                distance = rep1;
                            } else {
                                update_1!(prob, range, code, bound, ttt);
                                prob = probs.add(IS_REP_G2 + state as usize);
                                ttt = *prob as u32;
                                normalize!(range, code, buf);
                                bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
                                if code < bound {
                                    update_0!(prob, range, bound, ttt);
                                    distance = rep2;
                                } else {
                                    update_1!(prob, range, code, bound, ttt);
                                    distance = rep3;
                                    rep3 = rep2;
                                }
                                rep2 = rep1;
                            }
                            rep1 = rep0;
                            rep0 = distance;
                        }
                        state = if state < K_NUM_LIT_STATES { 8 } else { 11 };
                        prob = probs.add(REP_LEN_CODER);
                    }

                    {
                        let mut prob_len = prob.add(LEN_CHOICE);
                        ttt = *prob_len as u32;
                        normalize!(range, code, buf);
                        bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
                        if code < bound {
                            update_0!(prob_len, range, bound, ttt);
                            prob_len = prob.add(LEN_LOW + pos_state as usize);
                            len = 1;
                            tree_get_bit!(prob_len, len, ttt, bound, range, code, buf);
                            tree_get_bit!(prob_len, len, ttt, bound, range, code, buf);
                            tree_get_bit!(prob_len, len, ttt, bound, range, code, buf);
                            len -= 8;
                        } else {
                            update_1!(prob_len, range, code, bound, ttt);
                            prob_len = prob.add(LEN_CHOICE_2);
                            ttt = *prob_len as u32;
                            normalize!(range, code, buf);
                            bound = (range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
                            if code < bound {
                                update_0!(prob_len, range, bound, ttt);
                                prob_len = prob
                                    .add(LEN_LOW + pos_state as usize + (1 << K_LEN_NUM_LOW_BITS));
                                len = 1;
                                tree_get_bit!(prob_len, len, ttt, bound, range, code, buf);
                                tree_get_bit!(prob_len, len, ttt, bound, range, code, buf);
                                tree_get_bit!(prob_len, len, ttt, bound, range, code, buf);
                            } else {
                                update_1!(prob_len, range, code, bound, ttt);
                                prob_len = prob.add(LEN_HIGH);
                                // C: TREE_DECODE(probLen, (1 << kLenNumHighBits), len)
                                len = 1;
                                loop {
                                    tree_get_bit!(prob_len, len, ttt, bound, range, code, buf);
                                    if len >= (1 << K_LEN_NUM_HIGH_BITS) {
                                        break;
                                    }
                                }
                                len -= 1 << K_LEN_NUM_HIGH_BITS;
                                len += K_LEN_NUM_LOW_SYMBOLS * 2;
                            }
                        }
                    }

                    if state >= K_NUM_STATES {
                        let mut distance: u32;
                        {
                            let temp = (if len < K_NUM_LEN_TO_POS_STATES as u32 {
                                len
                            } else {
                                K_NUM_LEN_TO_POS_STATES as u32 - 1
                            }) << K_NUM_POS_SLOT_BITS;
                            prob = probs.add(POS_SLOT + temp as usize);
                        }
                        // C: TREE_6_DECODE(prob, distance)
                        distance = 1;
                        tree_get_bit!(prob, distance, ttt, bound, range, code, buf);
                        tree_get_bit!(prob, distance, ttt, bound, range, code, buf);
                        tree_get_bit!(prob, distance, ttt, bound, range, code, buf);
                        tree_get_bit!(prob, distance, ttt, bound, range, code, buf);
                        tree_get_bit!(prob, distance, ttt, bound, range, code, buf);
                        tree_get_bit!(prob, distance, ttt, bound, range, code, buf);
                        distance -= 0x40;

                        if distance >= K_START_POS_MODEL_INDEX {
                            let pos_slot = distance;
                            let mut num_direct_bits = (distance >> 1) - 1;
                            distance = 2 | (distance & 1);
                            if pos_slot < K_END_POS_MODEL_INDEX {
                                distance <<= num_direct_bits;
                                prob = probs.add(SPEC_POS);
                                {
                                    let mut m: u32 = 1;
                                    distance += 1;
                                    loop {
                                        rev_bit_var!(
                                            prob, distance, m, ttt, bound, range, code, buf
                                        );
                                        num_direct_bits -= 1;
                                        if num_direct_bits == 0 {
                                            break;
                                        }
                                    }
                                    distance -= m;
                                }
                            } else {
                                num_direct_bits -= K_NUM_ALIGN_BITS;
                                loop {
                                    normalize!(range, code, buf);
                                    range >>= 1;

                                    {
                                        code = code.wrapping_sub(range);
                                        /* (UInt32)((Int32)code >> 31) */
                                        let t: u32 = 0u32.wrapping_sub(code >> 31);
                                        distance = (distance << 1).wrapping_add(t.wrapping_add(1));
                                        code = code.wrapping_add(range & t);
                                    }
                                    /*
                                    distance <<= 1;
                                    if (code >= range)
                                    {
                                      code -= range;
                                      distance |= 1;
                                    }
                                    */
                                    num_direct_bits -= 1;
                                    if num_direct_bits == 0 {
                                        break;
                                    }
                                }
                                prob = probs.add(ALIGN);
                                distance <<= K_NUM_ALIGN_BITS;
                                {
                                    let mut i: u32 = 1;
                                    rev_bit_const!(prob, i, 1, ttt, bound, range, code, buf);
                                    rev_bit_const!(prob, i, 2, ttt, bound, range, code, buf);
                                    rev_bit_const!(prob, i, 4, ttt, bound, range, code, buf);
                                    rev_bit_last!(prob, i, 8, ttt, bound, range, code, buf);
                                    distance |= i;
                                }
                                if distance == 0xFFFF_FFFF {
                                    len = K_MATCH_SPEC_LEN_START;
                                    state -= K_NUM_STATES;
                                    break 'outer;
                                }
                            }
                        }

                        rep3 = rep2;
                        rep2 = rep1;
                        rep1 = rep0;
                        rep0 = distance.wrapping_add(1);
                        state = if state < K_NUM_STATES + K_NUM_LIT_STATES {
                            K_NUM_LIT_STATES
                        } else {
                            K_NUM_LIT_STATES + 3
                        };
                        if distance
                            >= (if check_dic_size == 0 {
                                processed_pos
                            } else {
                                check_dic_size
                            })
                        {
                            len += K_MATCH_SPEC_LEN_ERROR_DATA + K_MATCH_MIN_LEN;
                            break 'outer;
                        }
                    }

                    len += K_MATCH_MIN_LEN;

                    {
                        let rem: usize = limit - dic_pos;
                        if rem == 0 {
                            /*
                            We stop decoding and return SZ_OK, and we can resume decoding later.
                            Any error conditions can be tested later in caller code.
                            For more strict mode we can stop decoding with error
                            // len += kMatchSpecLen_Error_Data;
                            */
                            break 'outer;
                        }

                        let mut cur_len: usize = if rem < len as usize {
                            rem
                        } else {
                            len as usize
                        };
                        let mut pos: usize = dic_pos.wrapping_sub(rep0 as usize).wrapping_add(
                            if dic_pos < rep0 as usize {
                                dic_buf_size
                            } else {
                                0
                            },
                        );

                        processed_pos = processed_pos.wrapping_add(cur_len as u32);

                        len -= cur_len as u32;
                        if cur_len <= dic_buf_size - pos {
                            let mut dest = dic.add(dic_pos);
                            let src = (pos as isize) - (dic_pos as isize);
                            let lim = dest.add(cur_len);
                            dic_pos += cur_len;
                            loop {
                                *dest = *dest.offset(src);
                                dest = dest.add(1);
                                if dest == lim {
                                    break;
                                }
                            }
                        } else {
                            loop {
                                *dic.add(dic_pos) = *dic.add(pos);
                                dic_pos += 1;
                                pos += 1;
                                if pos == dic_buf_size {
                                    pos = 0;
                                }
                                cur_len -= 1;
                                if cur_len == 0 {
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            // C: `while (dicPos < limit && buf < bufLimit);`
            if !(dic_pos < limit && buf < buf_limit) {
                break 'outer;
            }
        }

        normalize!(range, code, buf);
    }

    p.range = range;
    p.code = code;
    p.remain_len = len; // & (kMatchSpecLen_Error_Data - 1); // we can write real length for error matches too.
    p.dic_pos = dic_pos;
    p.processed_pos = processed_pos;
    p.reps[0] = rep0;
    p.reps[1] = rep1;
    p.reps[2] = rep2;
    p.reps[3] = rep3;
    p.state = state;

    RealResult {
        buf,
        ok: len < K_MATCH_SPEC_LEN_ERROR_DATA,
    }
}
