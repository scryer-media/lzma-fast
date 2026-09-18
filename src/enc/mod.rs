//! The LZMA encoder.
//!
//! A faithful port of the encoder side of Igor Pavlov's LZMA SDK: `C/LzFind.c`
//! (the match finders), `C/LzmaEnc.c` (the range coder, the price tables and
//! the optimal parser) and `C/Lzma2Enc.c` (LZMA2 chunking). See
//! `docs/encoder.md` for what was left out and how parity is tested.

mod consts;
mod lz_find;
mod lzma_enc;
mod price;
mod props;
mod range_enc;
mod stream;

use alloc::vec::Vec;

use crate::error::Error;
use lzma_enc::LzmaEnc;

pub use consts::{LZMA_MATCH_LEN_MAX, LZMA_MATCH_LEN_MIN};
pub use lz_find::MatchFinderKind;
pub use props::LzmaEncProps;
pub use stream::{SeqInStream, SeqOutStream, SliceStream};

/// An LZMA1 encoder.
///
/// C: `CLzmaEncHandle`, driven by `LzmaEnc_Encode`.
///
/// The encoder holds its match finder window and probability model, so reusing
/// one across streams is much cheaper than building a new one — but each
/// [`LzmaEncoder::encode`] call re-initializes both, exactly as the C's
/// `LzmaEnc_Prepare` does, so consecutive streams are independent.
pub struct LzmaEncoder {
    inner: alloc::boxed::Box<LzmaEnc>,
}

impl LzmaEncoder {
    /// C: `LzmaEnc_Create` followed by `LzmaEnc_SetProps`.
    ///
    /// # Errors
    ///
    /// [`Error::Param`] if a setting is out of range, [`Error::Alloc`] if the
    /// encoder state could not be allocated.
    pub fn new(props: &LzmaEncProps) -> Result<Self, Error> {
        let mut inner = alloc::boxed::Box::new(LzmaEnc::new()?);
        inner.set_props(props)?;
        Ok(LzmaEncoder { inner })
    }

    /// The five LZMA property bytes a decoder needs for this setting.
    ///
    /// C: `LzmaEnc_WriteProperties`.
    #[must_use]
    pub fn properties(&self) -> [u8; crate::lzma::consts::LZMA_PROPS_SIZE] {
        self.inner.write_properties()
    }

    /// The dictionary size this encoder will use.
    #[must_use]
    pub fn dict_size(&self) -> u32 {
        self.inner.dict_size
    }

    /// Encode `input` into `out`.
    ///
    /// C: `LzmaEnc_Encode`. Whether the stream ends with an end marker is the
    /// `write_end_mark` setting; without one the decoder needs the uncompressed
    /// size from elsewhere.
    ///
    /// # Errors
    ///
    /// Whatever the streams return, or [`Error::Alloc`].
    pub fn encode(
        &mut self,
        input: &mut dyn SeqInStream,
        out: &mut dyn SeqOutStream,
    ) -> Result<(), Error> {
        self.inner.prepare(0)?;
        self.encode_prepared(input, out)
    }

    /// Encode `input` into `out`, telling the encoder how long the input is.
    ///
    /// C: `LzmaEnc_SetDataSize` before `LzmaEnc_Encode`, which is what
    /// `LzmaEnc_MemEncode` does implicitly. The size changes the output: it
    /// sizes the match finder's hash table, so an encoder told the size can
    /// find different matches from one that was not.
    ///
    /// # Errors
    ///
    /// As [`LzmaEncoder::encode`].
    pub fn encode_sized(
        &mut self,
        input: &mut dyn SeqInStream,
        out: &mut dyn SeqOutStream,
        input_len: u64,
    ) -> Result<(), Error> {
        self.inner.mem_prepare(input_len, 0)?;
        self.encode_prepared(input, out)
    }

    /// C: `LzmaEnc_Encode2`, without the progress callback.
    fn encode_prepared(
        &mut self,
        input: &mut dyn SeqInStream,
        out: &mut dyn SeqOutStream,
    ) -> Result<(), Error> {
        loop {
            self.inner.code_one_block(input, out, 0, 0)?;
            if self.inner.finished {
                return Ok(());
            }
        }
    }

    /// Encode a slice, returning the raw LZMA1 stream.
    ///
    /// C: `LzmaEnc_MemEncode`, which sets the data size from `srcLen`.
    ///
    /// # Errors
    ///
    /// [`Error::Alloc`] if the output could not be grown.
    pub fn encode_to_vec(&mut self, src: &[u8]) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        let mut input = SliceStream::new(src);
        self.encode_sized(&mut input, &mut out, src.len() as u64)?;
        Ok(out)
    }

    /// The number of input bytes encoded by the last call.
    #[must_use]
    pub fn processed_in(&self) -> u64 {
        self.inner.now_pos64
    }
}

/// Encode `src` as a `.lzma` (LZMA-Alone) file.
///
/// The 13-byte header is the five property bytes and the uncompressed size as
/// a little-endian 64-bit number, which is what
/// [`crate::LzmaAloneHeader::parse`] reads back.
///
/// # Errors
///
/// [`Error::Param`] if a setting is out of range, [`Error::Alloc`] on
/// allocation failure.
pub fn encode_lzma_alone(src: &[u8], props: &LzmaEncProps) -> Result<Vec<u8>, Error> {
    let mut enc = LzmaEncoder::new(props)?;
    let mut out = Vec::new();
    out.try_reserve(crate::lzma_alone::LZMA_ALONE_HEADER_SIZE)
        .map_err(|_| Error::Alloc)?;
    out.extend_from_slice(&enc.properties());
    out.extend_from_slice(&(src.len() as u64).to_le_bytes());
    let mut input = SliceStream::new(src);
    enc.encode_sized(&mut input, &mut out, src.len() as u64)?;
    Ok(out)
}
