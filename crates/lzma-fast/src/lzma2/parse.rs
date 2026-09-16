//! Walking an LZMA2 stream's chunk headers without decoding it.
//!
//! C: `Lzma2Dec_Parse` in `C/Lzma2Dec.c`, plus the `ELzma2ParseStatus`
//! extension of `ELzmaStatus` in `C/Lzma2Dec.h`.
//!
//! The reference drives this with the same `CLzma2Dec` the decoder uses, with
//! a null dictionary: the parse never touches dictionary *bytes*, only the
//! position, because all it needs is how much output each chunk will produce
//! and where the next independently decodable block starts. The port gives it
//! its own struct for the same reason, so that nothing about it can perturb
//! the single-threaded decoder.

use crate::lzma::LzmaProps;
use crate::lzma2::frame::{LZMA2_LCLP_MAX, Lzma2Frame, Lzma2State, is_uncompressed_state};

/// C: `ELzma2ParseStatus`, which is `ELzmaStatus` plus two codes of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseStatus {
    /// C: `LZMA_STATUS_NOT_SPECIFIED`, returned here only for a malformed
    /// stream.
    NotSpecified,
    /// C: `LZMA_STATUS_FINISHED_WITH_MARK`.
    FinishedWithMark,
    /// C: `LZMA_STATUS_NOT_FINISHED`.
    NotFinished,
    /// C: `LZMA_STATUS_NEEDS_MORE_INPUT`.
    NeedsMoreInput,
    /// C: `LZMA2_PARSE_STATUS_NEW_BLOCK`. A chunk that resets the dictionary
    /// starts here, so everything up to this point decodes independently of
    /// everything after it. The control byte has been consumed.
    NewBlock,
    /// C: `LZMA2_PARSE_STATUS_NEW_CHUNK`. A chunk header was read; the chunk's
    /// payload has not been walked yet.
    NewChunk,
}

/// Chunk-header walker over an LZMA2 stream.
///
/// C: the subset of `CLzma2Dec` that `Lzma2Dec_Parse` uses.
pub(crate) struct Lzma2Parser {
    frame: Lzma2Frame,
    /// C: `p->decoder.prop`. The parser keeps it only so that the
    /// `LZMA2_STATE_PROP` case can validate `lc`/`lp`/`pb` exactly as the
    /// decoder does.
    prop: LzmaProps,
    /// C: `p->decoder.dicPos`, advanced without a dictionary behind it.
    pub(crate) dic_pos: usize,
}

impl Lzma2Parser {
    pub(crate) fn new() -> Self {
        Lzma2Parser {
            frame: Lzma2Frame::new(),
            // lc = LZMA2_LCLP_MAX, lp = pb = 0: what Lzma2Dec_GetOldProps
            // hands LzmaDec_Allocate. Only `set_lclppb` ever changes it.
            prop: LzmaProps::new(LZMA2_LCLP_MAX, 0, 0, 1 << 12).expect("valid LZMA2 props"),
            dic_pos: 0,
        }
    }

    /// C: `Lzma2Dec_Init`, for a parser.
    pub(crate) fn init(&mut self) {
        self.frame.init();
        self.dic_pos = 0;
    }

    /// C: `p->unpackSize`, the declared output size of the chunk whose header
    /// was just read. Only meaningful right after [`ParseStatus::NewChunk`].
    pub(crate) fn unpack_size(&self) -> u32 {
        self.frame.unpack_size
    }

    /// C: `Lzma2Dec_GetUnpackExtra`. How much output the chunk currently being
    /// walked still owes, or zero if the parse is not inside one.
    pub(crate) fn unpack_extra(&self) -> u32 {
        if self.frame.is_extra_mode {
            self.frame.unpack_size
        } else {
            0
        }
    }

    /// C: `Lzma2Dec_Parse`.
    ///
    /// Walks chunk headers, advancing [`Self::dic_pos`] by what each chunk
    /// will produce, until `out_size` bytes of output have been accounted for,
    /// the input runs out, or a block boundary is reached. Returns the status
    /// and the number of input bytes consumed.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn parse(
        &mut self,
        mut out_size: usize,
        src: &[u8],
        check_finish_block: bool,
    ) -> (ParseStatus, usize) {
        let in_size = src.len();
        let mut src_len = 0usize;

        while self.frame.state != Lzma2State::Error {
            if self.frame.state == Lzma2State::Finished {
                return (ParseStatus::FinishedWithMark, src_len);
            }

            if out_size == 0 && !check_finish_block {
                return (ParseStatus::NotFinished, src_len);
            }

            if self.frame.state != Lzma2State::Data && self.frame.state != Lzma2State::DataCont {
                if src_len == in_size {
                    return (ParseStatus::NeedsMoreInput, src_len);
                }
                let b = src[src_len];
                src_len += 1;

                self.frame.state = self.frame.update_state(b, &mut self.prop);

                if self.frame.state == Lzma2State::Unpack0
                    && (self.frame.control == super::frame::LZMA2_CONTROL_COPY_RESET_DIC
                        || self.frame.control >= 0xE0)
                {
                    return (ParseStatus::NewBlock, src_len);
                }

                // The following code can be commented.
                // It's not big problem, if we read additional input bytes.
                // It will be stopped later in LZMA2_STATE_DATA /
                // LZMA2_STATE_DATA_CONT state.
                if out_size == 0 && self.frame.state != Lzma2State::Finished {
                    // checkFinishBlock is true. So we expect that block must be
                    // finished.
                    return (ParseStatus::NotFinished, src_len);
                }

                if self.frame.state == Lzma2State::Data {
                    return (ParseStatus::NewChunk, src_len);
                }

                continue;
            }

            if out_size == 0 {
                return (ParseStatus::NotFinished, src_len);
            }

            {
                let mut in_cur = in_size - src_len;

                if is_uncompressed_state(self.frame.control) {
                    if in_cur == 0 {
                        return (ParseStatus::NeedsMoreInput, src_len);
                    }
                    if in_cur > self.frame.unpack_size as usize {
                        in_cur = self.frame.unpack_size as usize;
                    }
                    if in_cur > out_size {
                        in_cur = out_size;
                    }
                    self.dic_pos += in_cur;
                    src_len += in_cur;
                    out_size -= in_cur;
                    self.frame.unpack_size -= in_cur as u32;
                    self.frame.state = if self.frame.unpack_size == 0 {
                        Lzma2State::Control
                    } else {
                        Lzma2State::DataCont
                    };
                } else {
                    self.frame.is_extra_mode = true;

                    if in_cur == 0 {
                        if self.frame.pack_size != 0 {
                            return (ParseStatus::NeedsMoreInput, src_len);
                        }
                    } else if self.frame.state == Lzma2State::Data {
                        self.frame.state = Lzma2State::DataCont;
                        if src[src_len] != 0 {
                            // first byte of lzma chunk must be Zero
                            src_len += 1;
                            self.frame.pack_size -= 1;
                            break;
                        }
                    }

                    if in_cur > self.frame.pack_size as usize {
                        in_cur = self.frame.pack_size as usize;
                    }

                    src_len += in_cur;
                    self.frame.pack_size -= in_cur as u32;

                    if self.frame.pack_size == 0 {
                        let mut rem = out_size;
                        if rem > self.frame.unpack_size as usize {
                            rem = self.frame.unpack_size as usize;
                        }
                        self.dic_pos += rem;
                        self.frame.unpack_size -= rem as u32;
                        out_size -= rem;
                        if self.frame.unpack_size == 0 {
                            self.frame.state = Lzma2State::Control;
                        }
                    }
                }
            }
        }

        self.frame.state = Lzma2State::Error;
        (ParseStatus::NotSpecified, src_len)
    }
}
