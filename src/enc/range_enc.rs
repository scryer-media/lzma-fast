//! The range encoder.
//!
//! C: `CRangeEnc` and the `RC_*` macros in `C/LzmaEnc.c`.
//!
//! One deviation, and it is only in where a value lives: the C holds `range`
//! in a local across a run of `RC_BIT`s and writes it back to `p->range` at
//! the end of each coding routine, because that is what its compiler needed.
//! Here `range` stays in the struct and the methods read and write it, which
//! produces the same bits.

use alloc::vec::Vec;

use crate::enc::consts::*;
use crate::enc::stream::SeqOutStream;
use crate::error::Error;

/// C: `CRangeEnc`.
pub(crate) struct RangeEnc {
    pub(crate) range: u32,
    cache: u32,
    low: u64,
    cache_size: u64,
    /// C: `bufBase`/`buf`/`bufLim`. The `Vec`'s length is the C's `buf`
    /// pointer and its capacity is `RC_BUF_SIZE`, the C's `bufLim`.
    buf: Vec<u8>,
    processed: u64,
    pub(crate) res: Result<(), Error>,
}

impl RangeEnc {
    /// C: `RangeEnc_Construct` plus `RangeEnc_Alloc`.
    pub(crate) fn new() -> Result<Self, Error> {
        let mut buf = Vec::new();
        buf.try_reserve_exact(RC_BUF_SIZE)
            .map_err(|_| Error::Alloc)?;
        Ok(RangeEnc {
            range: 0xFFFF_FFFF,
            cache: 0,
            low: 0,
            cache_size: 0,
            buf,
            processed: 0,
            res: Ok(()),
        })
    }

    /// C: `RangeEnc_Init`.
    pub(crate) fn init(&mut self) {
        self.range = 0xFFFF_FFFF;
        self.cache = 0;
        self.low = 0;
        self.cache_size = 0;
        self.buf.clear();
        self.processed = 0;
        self.res = Ok(());
    }

    /// C: `RangeEnc_GetProcessed`.
    pub(crate) fn get_processed(&self) -> u64 {
        self.processed + self.buf.len() as u64 + self.cache_size
    }

    /// C: `RangeEnc_FlushStream`.
    pub(crate) fn flush_stream(&mut self, out: &mut dyn SeqOutStream) {
        let num = self.buf.len();
        if self.res.is_ok()
            && let Err(e) = out.write(&self.buf)
        {
            self.res = Err(e);
        }
        self.processed += num as u64;
        self.buf.clear();
    }

    /// C: `RangeEnc_ShiftLow`.
    fn shift_low(&mut self, out: &mut dyn SeqOutStream) {
        let low = self.low as u32;
        let mut high = (self.low >> 32) as u32;
        self.low = u64::from(low << 8);
        if low < 0xFF00_0000 || high != 0 {
            self.push(self.cache.wrapping_add(high) as u8, out);
            self.cache = low >> 24;
            if self.cache_size == 0 {
                return;
            }
            high = high.wrapping_add(0xFF);
            loop {
                self.push(high as u8, out);
                self.cache_size -= 1;
                if self.cache_size == 0 {
                    return;
                }
            }
        }
        self.cache_size += 1;
    }

    #[inline]
    fn push(&mut self, byte: u8, out: &mut dyn SeqOutStream) {
        self.buf.push(byte);
        if self.buf.len() == RC_BUF_SIZE {
            self.flush_stream(out);
        }
    }

    /// C: `RangeEnc_FlushData`.
    pub(crate) fn flush_data(&mut self, out: &mut dyn SeqOutStream) {
        for _ in 0..5 {
            self.shift_low(out);
        }
    }

    /// C: `RC_NORM`.
    #[inline]
    fn norm(&mut self, out: &mut dyn SeqOutStream) {
        if self.range < K_TOP_VALUE {
            self.range <<= 8;
            self.shift_low(out);
        }
    }

    /// C: the `RC_BIT` macro, in its branchless form.
    #[inline]
    pub(crate) fn encode_bit(&mut self, prob: &mut u16, bit: u32, out: &mut dyn SeqOutStream) {
        // C: `RC_BIT_PRE`.
        let ttt = u32::from(*prob);
        let new_bound = (self.range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
        let mut mask = 0u32.wrapping_sub(bit);
        self.range &= mask;
        mask &= new_bound;
        self.range = self.range.wrapping_sub(mask);
        self.low = self.low.wrapping_add(u64::from(mask));
        let mut mask = bit.wrapping_sub(1);
        self.range = self.range.wrapping_add(new_bound & mask);
        mask &= K_BIT_MODEL_TOTAL - ((1 << K_NUM_MOVE_BITS) - 1);
        mask = mask.wrapping_add((1 << K_NUM_MOVE_BITS) - 1);
        let ttt = ttt.wrapping_add(((mask.wrapping_sub(ttt)) as i32 >> K_NUM_MOVE_BITS) as u32);
        *prob = ttt as u16;
        self.norm(out);
    }

    /// C: `RC_BIT_0_BASE`.
    #[inline]
    pub(crate) fn encode_bit_0_base(&mut self, prob: &mut u16) {
        let ttt = u32::from(*prob);
        let new_bound = (self.range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
        self.range = new_bound;
        *prob = (ttt + ((K_BIT_MODEL_TOTAL - ttt) >> K_NUM_MOVE_BITS)) as u16;
    }

    /// C: `RC_BIT_1_BASE`.
    #[inline]
    pub(crate) fn encode_bit_1_base(&mut self, prob: &mut u16) {
        let ttt = u32::from(*prob);
        let new_bound = (self.range >> K_NUM_BIT_MODEL_TOTAL_BITS) * ttt;
        self.range -= new_bound;
        self.low += u64::from(new_bound);
        *prob = (ttt - (ttt >> K_NUM_MOVE_BITS)) as u16;
    }

    /// C: `RC_BIT_0`, which is `RC_BIT_0_BASE` then `RC_NORM`.
    #[inline]
    pub(crate) fn encode_bit_0(&mut self, prob: &mut u16, out: &mut dyn SeqOutStream) {
        self.encode_bit_0_base(prob);
        self.norm(out);
    }

    /// C: `RC_BIT_1`.
    #[inline]
    pub(crate) fn encode_bit_1(&mut self, prob: &mut u16, out: &mut dyn SeqOutStream) {
        self.encode_bit_1_base(prob);
        self.norm(out);
    }

    /// C: the `RC_NORM` that follows a bare `RC_BIT_*_BASE` in
    /// `LzmaEnc_CodeOneBlock`.
    #[inline]
    pub(crate) fn norm_pub(&mut self, out: &mut dyn SeqOutStream) {
        self.norm(out);
    }

    /// C: the inlined `RangeEnc_EncodeDirectBits` in `LzmaEnc_CodeOneBlock`'s
    /// far-distance branch and in `WriteEndMarker`.
    #[inline]
    pub(crate) fn encode_direct_bit(&mut self, add: u32, out: &mut dyn SeqOutStream) {
        self.range >>= 1;
        self.low += u64::from(self.range & add);
        self.norm(out);
    }

    /// C: `LitEnc_Encode`.
    pub(crate) fn lit_encode(&mut self, probs: &mut [u16], sym: u32, out: &mut dyn SeqOutStream) {
        let mut sym = sym | 0x100;
        loop {
            let i = (sym >> 8) as usize;
            let bit = (sym >> 7) & 1;
            sym <<= 1;
            let mut prob = probs[i];
            self.encode_bit(&mut prob, bit, out);
            probs[i] = prob;
            if sym >= 0x10000 {
                break;
            }
        }
    }

    /// C: `LitEnc_EncodeMatched`.
    pub(crate) fn lit_encode_matched(
        &mut self,
        probs: &mut [u16],
        sym: u32,
        match_byte: u32,
        out: &mut dyn SeqOutStream,
    ) {
        let mut offs = 0x100u32;
        let mut sym = sym | 0x100;
        let mut match_byte = match_byte;
        loop {
            match_byte <<= 1;
            let i = (offs + (match_byte & offs) + (sym >> 8)) as usize;
            let bit = (sym >> 7) & 1;
            sym <<= 1;
            offs &= !(match_byte ^ sym);
            let mut prob = probs[i];
            self.encode_bit(&mut prob, bit, out);
            probs[i] = prob;
            if sym >= 0x10000 {
                break;
            }
        }
    }

    /// C: `RcTree_ReverseEncode`.
    pub(crate) fn rc_tree_reverse_encode(
        &mut self,
        probs: &mut [u16],
        num_bits: u32,
        sym: u32,
        out: &mut dyn SeqOutStream,
    ) {
        let mut m = 1usize;
        let mut sym = sym;
        for _ in 0..num_bits {
            let bit = sym & 1;
            sym >>= 1;
            let mut prob = probs[m];
            self.encode_bit(&mut prob, bit, out);
            probs[m] = prob;
            m = (m << 1) | bit as usize;
        }
    }
}
