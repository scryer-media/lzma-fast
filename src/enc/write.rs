//! `std::io::Write` adapters over the encoders.
//!
//! The decoder side offers [`crate::LzmaReader`], [`crate::xz::XzReader`] and
//! friends as `Read`; these are the mirror image, so that a caller can pipe
//! into a compressor the same way it pipes out of a decompressor.
//!
//! All three buffer their whole input. That is not laziness about streaming:
//! the `.lzma` header carries the uncompressed size, LZMA2's chunking wants
//! the data size in front of it because it sizes the match finder's hash
//! table, and a block header declares both of its sizes — none of which is
//! known until the writer is closed. The `.xz` writer does drop each block as
//! it fills, so a caller that sets a block size bounds what is held.

use std::io::{self, Write};

use alloc::vec::Vec;

use crate::error::Error;

use super::lzma2_enc::Lzma2Encoder;
use super::props::LzmaEncProps;

#[cfg(feature = "xz")]
use super::xz_enc::XzEncoder;
#[cfg(feature = "xz")]
use crate::xz::stream::CheckType;

/// Turns an encoder error into the `io::Error` a `Write` must return.
fn io(e: Error) -> io::Error {
    io::Error::other(e)
}

/// Writes a `.lzma` (LZMA-Alone) file.
///
/// The 13-byte header carries the uncompressed size, so nothing can be
/// written until [`LzmaWriter::finish`] is called.
pub struct LzmaWriter<W: Write> {
    inner: Option<W>,
    props: LzmaEncProps,
    buf: Vec<u8>,
}

impl<W: Write> LzmaWriter<W> {
    /// A writer that will compress everything pushed into it with `props`.
    #[must_use]
    pub fn new(inner: W, props: &LzmaEncProps) -> Self {
        LzmaWriter {
            inner: Some(inner),
            props: *props,
            buf: Vec::new(),
        }
    }

    /// Compresses everything written so far, writes it out, and returns the
    /// wrapped writer.
    ///
    /// # Errors
    ///
    /// Whatever the encoder or the wrapped writer returns.
    pub fn finish(mut self) -> io::Result<W> {
        let mut inner = self.inner.take().expect("finish once");
        let out = super::encode_lzma_alone(&self.buf, &self.props).map_err(io)?;
        inner.write_all(&out)?;
        inner.flush()?;
        Ok(inner)
    }
}

impl<W: Write> Write for LzmaWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf
            .try_reserve(buf.len())
            .map_err(|_| io(Error::Alloc))?;
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Writes a raw LZMA2 stream, the payload an `.xz` block or a 7z coder holds.
///
/// [`Lzma2Writer::properties`] is the single dictionary property byte the
/// container has to carry alongside it.
pub struct Lzma2Writer<W: Write> {
    inner: Option<W>,
    enc: Lzma2Encoder,
    buf: Vec<u8>,
}

impl<W: Write> Lzma2Writer<W> {
    /// A writer that will compress everything pushed into it with `props`.
    ///
    /// # Errors
    ///
    /// [`Error::Param`] if a setting is out of range or `lc + lp` is above 4.
    pub fn new(inner: W, props: &LzmaEncProps) -> Result<Self, Error> {
        Ok(Lzma2Writer {
            inner: Some(inner),
            enc: Lzma2Encoder::new(props)?,
            buf: Vec::new(),
        })
    }

    /// The single LZMA2 property byte a decoder needs.
    #[must_use]
    pub fn properties(&self) -> u8 {
        self.enc.properties()
    }

    /// Compresses everything written so far, writes it out, and returns the
    /// wrapped writer.
    ///
    /// # Errors
    ///
    /// Whatever the encoder or the wrapped writer returns.
    pub fn finish(mut self) -> io::Result<W> {
        let mut inner = self.inner.take().expect("finish once");
        let out = self.enc.encode_to_vec(&self.buf).map_err(io)?;
        inner.write_all(&out)?;
        inner.flush()?;
        Ok(inner)
    }
}

impl<W: Write> Write for Lzma2Writer<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf
            .try_reserve(buf.len())
            .map_err(|_| io(Error::Alloc))?;
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Writes an `.xz` stream.
///
/// Unlike the other two this one really does stream: each block is compressed
/// and written out as it fills, so the memory it holds is bounded by
/// [`XzWriter::set_block_size`] plus the index.
#[cfg(feature = "xz")]
pub struct XzWriter<W: Write> {
    inner: Option<W>,
    enc: XzEncoder,
}

#[cfg(feature = "xz")]
impl<W: Write> XzWriter<W> {
    /// A writer with the given LZMA2 settings, a CRC-64 check and one block
    /// for the whole input.
    ///
    /// # Errors
    ///
    /// [`Error::Param`] if a setting is out of range or `lc + lp` is above 4.
    pub fn new(inner: W, props: &LzmaEncProps) -> Result<Self, Error> {
        Ok(XzWriter {
            inner: Some(inner),
            enc: XzEncoder::new(props)?,
        })
    }

    /// Sets the per-block check. Must be called before anything is written.
    ///
    /// # Errors
    ///
    /// [`Error::Param`] for a check this build cannot compute, or if bytes
    /// have already gone in.
    pub fn set_check(&mut self, check: CheckType) -> Result<(), Error> {
        self.enc.set_check(check)
    }

    /// Sets how much one block may decode to. Zero means one block for
    /// everything.
    pub fn set_block_size(&mut self, bytes: u64) {
        self.enc.set_block_size(bytes);
    }

    /// Writes the index and footer and returns the wrapped writer.
    ///
    /// # Errors
    ///
    /// Whatever the encoder or the wrapped writer returns.
    pub fn finish(mut self) -> io::Result<W> {
        let mut inner = self.inner.take().expect("finish once");
        self.enc.finish().map_err(io)?;
        inner.write_all(&self.enc.take_output())?;
        inner.flush()?;
        Ok(inner)
    }

    /// Hands whatever the encoder has produced to the wrapped writer.
    fn drain(&mut self) -> io::Result<()> {
        if self.enc.output().is_empty() {
            return Ok(());
        }
        let out = self.enc.take_output();
        self.inner.as_mut().expect("open").write_all(&out)
    }
}

#[cfg(feature = "xz")]
impl<W: Write> Write for XzWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.enc.push(buf).map_err(io)?;
        self.drain()?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.drain()?;
        self.inner.as_mut().expect("open").flush()
    }
}
