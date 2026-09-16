//! LZMA2 decoding: chunk framing over the LZMA1 decoder.
//!
//! C: `C/Lzma2Dec.c`. `Lzma2Dec_Parse` and everything it exists for
//! (`Lzma2DecMt.c`, `MtDec.c`) are out of scope: this crate is
//! single-threaded, so nothing needs to scan ahead for block boundaries.
//!
//! ```text
//! 00000000  -  End of data
//! 00000001 U U  -  Uncompressed, reset dic, need reset state and set new prop
//! 00000010 U U  -  Uncompressed, no reset
//! 100uuuuu U U P P  -  LZMA, no reset
//! 101uuuuu U U P P  -  LZMA, reset state
//! 110uuuuu U U P P S  -  LZMA, reset state + set new prop
//! 111uuuuu U U P P S  -  LZMA, reset state + set new prop, reset dic
//!
//!   u, U - Unpack Size
//!   P - Pack Size
//!   S - Props
//! ```

use crate::error::{Error, FinishMode, Progress, Status};
use crate::lzma::consts::LZMA_DIC_MIN;
use crate::lzma::{LzmaDec, LzmaProps, lzma_dec_decode_to_dic};

/// C: `LZMA2_CONTROL_COPY_RESET_DIC`.
const LZMA2_CONTROL_COPY_RESET_DIC: u8 = 1;
/// C: `LZMA2_LCLP_MAX`.
const LZMA2_LCLP_MAX: u8 = 4;

/// C: `LZMA2_IS_UNCOMPRESSED_STATE(p)`.
const fn is_uncompressed_state(control: u8) -> bool {
    (control & (1 << 7)) == 0
}

/// C: `LZMA2_DIC_SIZE_FROM_PROP(p)`.
const fn dic_size_from_prop(prop: u8) -> u32 {
    (2u32 | (prop as u32 & 1)) << (prop as u32 / 2 + 11)
}

/// C: `ELzma2State`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lzma2State {
    Control,
    Unpack0,
    Unpack1,
    Pack0,
    Pack1,
    Prop,
    Data,
    DataCont,
    Finished,
    Error,
}

/// LZMA2 decoder.
///
/// C: `CLzma2Dec`.
pub struct Lzma2Decoder {
    state: Lzma2State,
    control: u8,
    need_init_level: u8,
    unpack_size: u32,
    pack_size: u32,
    decoder: LzmaDec,
}

impl Lzma2Decoder {
    /// Allocates a decoder for the LZMA2 dictionary-size property byte, the
    /// single byte an xz filter flag or a 7z coder carries.
    ///
    /// C: `Lzma2Dec_Allocate` + `Lzma2Dec_Init`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedProps`] if `dict_prop > 40`, and
    /// [`Error::Alloc`] if the dictionary cannot be allocated.
    pub fn new(dict_prop: u8) -> Result<Self, Error> {
        // C: Lzma2Dec_GetOldProps
        if dict_prop > 40 {
            return Err(Error::UnsupportedProps);
        }
        let dic_size = if dict_prop == 40 {
            0xFFFF_FFFF
        } else {
            dic_size_from_prop(dict_prop)
        };
        // C: props[0] = LZMA2_LCLP_MAX, i.e. lc = 4, lp = 0, pb = 0, which
        // sizes the probability table for the largest (lc + lp) LZMA2 allows.
        let prop = LzmaProps::new(LZMA2_LCLP_MAX, 0, 0, dic_size.max(LZMA_DIC_MIN))?;
        let mut p = Lzma2Decoder {
            state: Lzma2State::Control,
            control: 0,
            need_init_level: 0xE0,
            unpack_size: 0,
            pack_size: 0,
            decoder: LzmaDec::new(prop)?,
        };
        p.reset();
        Ok(p)
    }

    /// C: `Lzma2Dec_Init`.
    pub fn reset(&mut self) {
        self.state = Lzma2State::Control;
        self.need_init_level = 0xE0;
        self.unpack_size = 0;
        self.decoder.init();
    }

    /// C: `Lzma2Dec_UpdateState`.
    fn update_state(&mut self, b: u8) -> Lzma2State {
        match self.state {
            Lzma2State::Control => {
                self.control = b;
                if b == 0 {
                    return Lzma2State::Finished;
                }
                if is_uncompressed_state(b) {
                    if b == LZMA2_CONTROL_COPY_RESET_DIC {
                        self.need_init_level = 0xC0;
                    } else if b > 2 || self.need_init_level == 0xE0 {
                        return Lzma2State::Error;
                    }
                } else {
                    if b < self.need_init_level {
                        return Lzma2State::Error;
                    }
                    self.need_init_level = 0;
                    self.unpack_size = u32::from(b & 0x1F) << 16;
                }
                Lzma2State::Unpack0
            }

            Lzma2State::Unpack0 => {
                self.unpack_size |= u32::from(b) << 8;
                Lzma2State::Unpack1
            }

            Lzma2State::Unpack1 => {
                self.unpack_size |= u32::from(b);
                self.unpack_size += 1;
                if is_uncompressed_state(self.control) {
                    Lzma2State::Data
                } else {
                    Lzma2State::Pack0
                }
            }

            Lzma2State::Pack0 => {
                self.pack_size = u32::from(b) << 8;
                Lzma2State::Pack1
            }

            Lzma2State::Pack1 => {
                self.pack_size |= u32::from(b);
                self.pack_size += 1;
                if self.control & 0x40 != 0 {
                    Lzma2State::Prop
                } else {
                    Lzma2State::Data
                }
            }

            Lzma2State::Prop => {
                let mut b = b;
                if b >= (9 * 5 * 5) {
                    return Lzma2State::Error;
                }
                let lc = b % 9;
                b /= 9;
                let pb = b / 5;
                let lp = b % 5;
                if lc + lp > LZMA2_LCLP_MAX {
                    return Lzma2State::Error;
                }
                self.decoder.prop.set_lclppb(lc, lp, pb);
                Lzma2State::Data
            }

            _ => Lzma2State::Error,
        }
    }

    /// C: `Lzma2Dec_DecodeToDic`.
    #[allow(clippy::too_many_lines)]
    fn decode_to_dic(
        &mut self,
        dic_limit: usize,
        src: &[u8],
        finish_mode: FinishMode,
        status: &mut Status,
    ) -> Result<usize, Error> {
        let in_size = src.len();
        let mut src_len = 0usize;
        *status = Status::NotSpecified;

        while self.state != Lzma2State::Error {
            if self.state == Lzma2State::Finished {
                *status = Status::FinishedWithMark;
                return Ok(src_len);
            }

            let dic_pos = self.decoder.dic_pos;

            if dic_pos == dic_limit && finish_mode == FinishMode::Any {
                *status = Status::NotFinished;
                return Ok(src_len);
            }

            if self.state != Lzma2State::Data && self.state != Lzma2State::DataCont {
                if src_len == in_size {
                    *status = Status::NeedsMoreInput;
                    return Ok(src_len);
                }
                let b = src[src_len];
                src_len += 1;
                self.state = self.update_state(b);
                if dic_pos == dic_limit && self.state != Lzma2State::Finished {
                    break;
                }
                continue;
            }

            {
                let mut in_cur = in_size - src_len;
                let mut out_cur = dic_limit - dic_pos;
                let mut cur_finish_mode = FinishMode::Any;

                if out_cur >= self.unpack_size as usize {
                    out_cur = self.unpack_size as usize;
                    cur_finish_mode = FinishMode::End;
                }

                if is_uncompressed_state(self.control) {
                    if in_cur == 0 {
                        *status = Status::NeedsMoreInput;
                        return Ok(src_len);
                    }

                    if self.state == Lzma2State::Data {
                        let init_dic = self.control == LZMA2_CONTROL_COPY_RESET_DIC;
                        self.decoder.init_dic_and_state(init_dic, false);
                    }

                    if in_cur > out_cur {
                        in_cur = out_cur;
                    }
                    if in_cur == 0 {
                        break;
                    }

                    // C: LzmaDec_UpdateWithUncompressed
                    let p = &mut self.decoder;
                    p.dic[p.dic_pos..p.dic_pos + in_cur]
                        .copy_from_slice(&src[src_len..src_len + in_cur]);
                    p.dic_pos += in_cur;
                    if p.check_dic_size == 0
                        && p.prop.dict_size() - p.processed_pos <= in_cur as u32
                    {
                        p.check_dic_size = p.prop.dict_size();
                    }
                    p.processed_pos = p.processed_pos.wrapping_add(in_cur as u32);

                    src_len += in_cur;
                    self.unpack_size -= in_cur as u32;
                    self.state = if self.unpack_size == 0 {
                        Lzma2State::Control
                    } else {
                        Lzma2State::DataCont
                    };
                } else {
                    if self.state == Lzma2State::Data {
                        let init_dic = self.control >= 0xE0;
                        let init_state = self.control >= 0xA0;
                        self.decoder.init_dic_and_state(init_dic, init_state);
                        self.state = Lzma2State::DataCont;
                    }

                    if in_cur > self.pack_size as usize {
                        in_cur = self.pack_size as usize;
                    }

                    let res = lzma_dec_decode_to_dic(
                        &mut self.decoder,
                        dic_pos + out_cur,
                        &src[src_len..src_len + in_cur],
                        cur_finish_mode,
                        status,
                    );

                    let in_cur = res?;

                    src_len += in_cur;
                    self.pack_size -= in_cur as u32;
                    let out_cur = self.decoder.dic_pos - dic_pos;
                    self.unpack_size -= out_cur as u32;

                    if *status == Status::NeedsMoreInput {
                        if self.pack_size == 0 {
                            break;
                        }
                        return Ok(src_len);
                    }

                    if in_cur == 0 && out_cur == 0 {
                        if *status != Status::MaybeFinishedWithoutMark
                            || self.unpack_size != 0
                            || self.pack_size != 0
                        {
                            break;
                        }
                        self.state = Lzma2State::Control;
                    }

                    *status = Status::NotSpecified;
                }
            }
        }

        *status = Status::NotSpecified;
        self.state = Lzma2State::Error;
        Err(Error::CorruptData)
    }

    /// C: `Lzma2Dec_DecodeToBuf`.
    ///
    /// Consumes bytes from `input`, writes decoded bytes to `output`, and
    /// reports where it stopped.
    ///
    /// # Errors
    ///
    /// Returns [`Error::CorruptData`] for malformed input.
    pub fn decode(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        finish: FinishMode,
    ) -> Result<Progress, Error> {
        let mut out_size = output.len();
        let mut src_pos = 0usize;
        let mut dest_pos = 0usize;
        let mut status = Status::NotSpecified;

        loop {
            if self.decoder.dic_pos == self.decoder.dic_buf_size {
                self.decoder.dic_pos = 0;
            }
            let dic_pos = self.decoder.dic_pos;
            let mut cur_finish_mode = FinishMode::Any;
            let mut out_cur = self.decoder.dic_buf_size - dic_pos;

            if out_cur >= out_size {
                out_cur = out_size;
                cur_finish_mode = finish;
            }

            let in_cur = self.decode_to_dic(
                dic_pos + out_cur,
                &input[src_pos..],
                cur_finish_mode,
                &mut status,
            )?;

            src_pos += in_cur;
            let out_cur = self.decoder.dic_pos - dic_pos;
            output[dest_pos..dest_pos + out_cur]
                .copy_from_slice(&self.decoder.dic[dic_pos..dic_pos + out_cur]);
            dest_pos += out_cur;
            out_size -= out_cur;

            if out_cur == 0 || out_size == 0 {
                return Ok(Progress {
                    read: src_pos,
                    written: dest_pos,
                    status,
                });
            }
        }
    }
}
