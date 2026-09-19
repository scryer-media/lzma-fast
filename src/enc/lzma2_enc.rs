//! The LZMA2 encoder.
//!
//! C: `CLzma2Enc` in `C/Lzma2Enc.c`, in its single-threaded, solid-block form:
//! `Lzma2EncInt_EncodeSubblock`, `Lzma2Enc_EncodeMt1`'s `outStream` path, and
//! `Lzma2Enc_WriteProperties`.
//!
//! What is not ported: `MtCoder.h` and everything `Z7_ST` guards, and with it
//! `blockSize` other than solid. `Lzma2EncProps_Normalize`'s block-size
//! arithmetic exists only to divide the input between block threads, so a
//! single-threaded encoder that always encodes one solid block needs none of
//! it. `docs/encoder.md` says what adding threads would take.

use alloc::vec::Vec;

use crate::enc::lzma_enc::LzmaEnc;
use crate::enc::props::LzmaEncProps;
use crate::enc::stream::{LimitedSeqInStream, SeqInStream, SeqOutStream, SliceStream};
use crate::error::Error;

/// C: `LZMA2_CONTROL_LZMA`.
const CONTROL_LZMA: u8 = 1 << 7;
/// C: `LZMA2_CONTROL_COPY_NO_RESET`.
const CONTROL_COPY_NO_RESET: u8 = 2;
/// C: `LZMA2_CONTROL_COPY_RESET_DIC`.
const CONTROL_COPY_RESET_DIC: u8 = 1;
/// C: `LZMA2_CONTROL_EOF`.
const CONTROL_EOF: u8 = 0;

/// C: `LZMA2_PACK_SIZE_MAX`, which is also `LZMA2_COPY_CHUNK_SIZE`.
const PACK_SIZE_MAX: usize = 1 << 16;
/// C: `LZMA2_UNPACK_SIZE_MAX`, which is also `LZMA2_KEEP_WINDOW_SIZE`.
const UNPACK_SIZE_MAX: u32 = 1 << 21;
/// C: `LZMA2_CHUNK_SIZE_COMPRESSED_MAX`.
const CHUNK_SIZE_COMPRESSED_MAX: usize = (1 << 16) + 16;

/// C: `LZMA2_DIC_SIZE_FROM_PROP(p)`.
const fn dic_size_from_prop(p: u32) -> u32 {
    (2 | (p & 1)) << (p / 2 + 11)
}

/// An LZMA2 encoder: one solid block of LZMA2 chunks, ending in the
/// end-of-stream control byte.
///
/// C: `CLzma2EncHandle` driven by `Lzma2Enc_Encode2`.
pub struct Lzma2Encoder {
    /// C: `CLzma2EncInt`, whose one `enc` this is.
    enc: alloc::boxed::Box<LzmaEnc>,
    props_byte: u8,
    dict_size: u32,
    /// C: `p->needInitState`.
    need_init_state: bool,
    /// C: `p->needInitProp`.
    need_init_prop: bool,
    /// C: `p->srcPos`, the block's uncompressed position.
    src_pos: u64,
    /// C: `me->tempBufLzma`.
    temp: Vec<u8>,
    /// C: `me->expectedDataSize`.
    expected_data_size: u64,
}

impl Lzma2Encoder {
    /// C: `Lzma2Enc_Create` plus `Lzma2Enc_SetProps`, with
    /// `Lzma2EncInt_InitStream`'s property capture folded in.
    ///
    /// # Errors
    ///
    /// [`Error::Param`] if a setting is out of range — including `lc + lp`
    /// above 4, which LZMA2 does not allow — and [`Error::Alloc`] on
    /// allocation failure.
    pub fn new(props: &LzmaEncProps) -> Result<Self, Error> {
        // C: `Lzma2Enc_SetProps` refuses lc + lp above `LZMA2_LCLP_MAX`,
        // which is what an LZMA2 decoder is allowed to allocate for.
        props.check_lclp_for_lzma2()?;

        let mut enc = alloc::boxed::Box::new(LzmaEnc::new()?);
        enc.set_props(props)?;
        let props_byte = enc.write_properties()[0];
        let dict_size = enc.dict_size;

        let mut temp = Vec::new();
        temp.try_reserve_exact(CHUNK_SIZE_COMPRESSED_MAX)
            .map_err(|_| Error::Alloc)?;

        Ok(Lzma2Encoder {
            enc,
            props_byte,
            dict_size,
            need_init_state: true,
            need_init_prop: true,
            src_pos: 0,
            temp,
            expected_data_size: u64::MAX,
        })
    }

    /// The single LZMA2 property byte, as the `.xz` filter and the 7z coder
    /// carry it.
    ///
    /// C: `Lzma2Enc_WriteProperties`.
    #[must_use]
    pub fn properties(&self) -> u8 {
        let mut i = 0u32;
        while i < 40 {
            if self.dict_size <= dic_size_from_prop(i) {
                break;
            }
            i += 1;
        }
        i as u8
    }

    /// The dictionary size the property byte rounds up to.
    #[must_use]
    pub fn dict_size(&self) -> u32 {
        self.dict_size
    }

    /// C: `Lzma2Enc_SetDataSize`.
    pub fn set_data_size(&mut self, expected: u64) {
        self.expected_data_size = expected;
    }

    /// C: `Lzma2EncInt_InitBlock`.
    fn init_block(&mut self) {
        self.src_pos = 0;
        self.need_init_state = true;
        self.need_init_prop = true;
    }

    /// Encode `input` into `out` as one LZMA2 stream.
    ///
    /// C: `Lzma2Enc_Encode2`'s `outStream` path, with `blockSize` solid, so
    /// there is exactly one block and the loop over blocks runs once.
    ///
    /// # Errors
    ///
    /// Whatever the streams return, or [`Error::Alloc`].
    pub fn encode(
        &mut self,
        input: &mut dyn SeqInStream,
        out: &mut dyn SeqOutStream,
    ) -> Result<(), Error> {
        self.init_block();

        let mut limited = LimitedSeqInStream::new(input);
        self.enc.set_data_size(self.expected_data_size);
        self.enc.prepare(UNPACK_SIZE_MAX)?;

        loop {
            let pack_size = self.encode_subblock(&mut limited, out)?;
            if pack_size == 0 {
                break;
            }
        }

        if self.src_pos != limited.processed {
            return Err(Error::InternalFailure);
        }

        out.write(&[CONTROL_EOF])
    }

    /// Encode a slice into one LZMA2 stream.
    ///
    /// # Errors
    ///
    /// [`Error::Alloc`] if the output could not be grown.
    pub fn encode_to_vec(&mut self, src: &[u8]) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        let mut input = SliceStream::new(src);
        self.set_data_size(src.len() as u64);
        self.encode(&mut input, &mut out)?;
        Ok(out)
    }

    /// C: `Lzma2EncInt_EncodeSubblock`, in the `outStream` form. Returns how
    /// many bytes it wrote, which is zero when the block is finished.
    fn encode_subblock(
        &mut self,
        input: &mut dyn SeqInStream,
        out: &mut dyn SeqOutStream,
    ) -> Result<usize, Error> {
        let lz_header_size = 5 + usize::from(self.need_init_prop);
        let mut unpack_size = UNPACK_SIZE_MAX;

        self.enc.save_state();
        self.temp.clear();
        let res = self.enc.code_one_mem_block(
            input,
            self.need_init_state,
            &mut self.temp,
            CHUNK_SIZE_COMPRESSED_MAX - lz_header_size,
            PACK_SIZE_MAX,
            &mut unpack_size,
        );
        // C: an output overflow is not an error here — it is the signal to
        // store the chunk instead.
        let overflowed = res?;
        let pack_size = self.temp.len();

        if unpack_size == 0 {
            return Ok(0);
        }

        // C: the chunk did not pay for itself (or did not fit), so store it.
        if overflowed || pack_size + 2 >= unpack_size as usize || pack_size > (1 << 16) {
            let mut written = 0usize;
            let mut remaining = unpack_size;
            // C: `LzmaEnc_GetCurBuf(p->enc) - unpackSize`.
            let mut at = self.enc.get_cur_buf() - unpack_size as usize;
            while remaining != 0 {
                let u = remaining.min(PACK_SIZE_MAX as u32);
                let control = if self.src_pos == 0 {
                    CONTROL_COPY_RESET_DIC
                } else {
                    CONTROL_COPY_NO_RESET
                };
                out.write(&[control, ((u - 1) >> 8) as u8, (u - 1) as u8])?;
                out.write(&self.enc.window()[at..at + u as usize])?;
                at += u as usize;
                remaining -= u;
                self.src_pos += u64::from(u);
                written += 3 + u as usize;
            }
            self.enc.restore_state();
            return Ok(written);
        }

        let u = unpack_size - 1;
        let pm = (pack_size - 1) as u32;
        // C: 3 resets the dictionary, 2 resets state and properties, 1 resets
        // state only, 0 continues.
        let mode: u8 = if self.src_pos == 0 {
            3
        } else if self.need_init_state {
            if self.need_init_prop { 2 } else { 1 }
        } else {
            0
        };

        let mut header = [0u8; 6];
        header[0] = CONTROL_LZMA | (mode << 5) | ((u >> 16) & 0x1F) as u8;
        header[1] = (u >> 8) as u8;
        header[2] = u as u8;
        header[3] = (pm >> 8) as u8;
        header[4] = pm as u8;
        if self.need_init_prop {
            header[5] = self.props_byte;
        }
        out.write(&header[..lz_header_size])?;
        out.write(&self.temp)?;

        self.need_init_prop = false;
        self.need_init_state = false;
        self.src_pos += u64::from(unpack_size);
        Ok(lz_header_size + pack_size)
    }
}

/// Encode `src` as a raw LZMA2 stream, returning it with its property byte.
///
/// # Errors
///
/// [`Error::Param`] if a setting is out of range, [`Error::Alloc`] on
/// allocation failure.
pub fn encode_lzma2(src: &[u8], props: &LzmaEncProps) -> Result<(u8, Vec<u8>), Error> {
    let mut enc = Lzma2Encoder::new(props)?;
    let out = enc.encode_to_vec(src)?;
    Ok((enc.properties(), out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_byte_rounds_the_dictionary_up() {
        // C: `LZMA2_DIC_SIZE_FROM_PROP(0)` is 1 << 12 and each step is the
        // next 2 or 3 times a power of two.
        assert_eq!(dic_size_from_prop(0), 1 << 12);
        assert_eq!(dic_size_from_prop(1), 3 << 11);
        assert_eq!(dic_size_from_prop(40 - 1), 3 << 30);
        for prop in 0..40u32 {
            let size = dic_size_from_prop(prop);
            let enc = Lzma2Encoder::new(&LzmaEncProps::new().with_dict_size(size)).unwrap();
            assert_eq!(u32::from(enc.properties()), prop, "dict size {size}");
        }
    }

    #[test]
    fn a_vec_of_zeros_becomes_one_lzma_chunk_and_an_end_marker() {
        let (_prop, out) = encode_lzma2(&[0u8; 4096], &LzmaEncProps::new()).unwrap();
        assert_eq!(out[0] & CONTROL_LZMA, CONTROL_LZMA, "an LZMA chunk");
        assert_eq!(out[0] >> 5 & 3, 3, "the first chunk resets the dictionary");
        assert_eq!(*out.last().unwrap(), CONTROL_EOF);
    }

    #[test]
    fn incompressible_input_falls_back_to_stored_chunks() {
        // A stored chunk is what the C writes when the packed size did not beat
        // the unpacked one; random bytes are the case that forces it.
        let mut x = 0x1234_5678u32;
        let src: Vec<u8> = (0..200_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        let (_prop, out) = encode_lzma2(&src, &LzmaEncProps::new()).unwrap();
        assert!(
            out.contains(&CONTROL_COPY_NO_RESET) || out[0] == CONTROL_COPY_RESET_DIC,
            "expected at least one stored chunk"
        );
        assert!(out.len() > src.len() / 2);
    }

    /// Reusing one encoder for several independent streams must not carry
    /// state across, whatever the sizes are.
    #[test]
    fn reuse_across_streams_of_different_sizes() {
        let props = LzmaEncProps::new().with_dict_size(1 << 16);
        let mut enc = Lzma2Encoder::new(&props).expect("new");
        let src: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
        let _ = enc.encode_to_vec(&src).expect("whole");
        for chunk in src.chunks(16 * 1024) {
            let _ = enc.encode_to_vec(chunk).expect("encode");
        }
    }
}
