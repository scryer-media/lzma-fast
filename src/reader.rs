//! `std::io::Read` adapters over the decoders.
//!
//! C: `C/Util/Lzma/LzmaUtil.c`'s `Decode2` streaming loop, expressed as a
//! `Read` implementation instead of a push loop.

use std::io::{self, Read};

use crate::error::{Error, FinishMode, Status};
use crate::lzma::{LzmaDecoder, LzmaProps};
use crate::lzma_alone::{LZMA_ALONE_HEADER_SIZE, LzmaAloneHeader};
use crate::lzma2::Lzma2Decoder;

/// Input buffer size for the adapters. Large enough that the per-call overhead
/// of the decoder entry point disappears against the work it does.
const IN_BUF_SIZE: usize = 1 << 16;

fn to_io(e: Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

struct Source<R> {
    inner: R,
    buf: Box<[u8]>,
    pos: usize,
    len: usize,
    eof: bool,
}

impl<R: Read> Source<R> {
    fn new(inner: R) -> Self {
        Source {
            inner,
            buf: vec![0u8; IN_BUF_SIZE].into_boxed_slice(),
            pos: 0,
            len: 0,
            eof: false,
        }
    }

    /// Ensures at least one buffered byte if the underlying reader has any.
    fn fill(&mut self) -> io::Result<()> {
        if self.pos == self.len && !self.eof {
            self.pos = 0;
            self.len = 0;
            let n = self.inner.read(&mut self.buf)?;
            if n == 0 {
                self.eof = true;
            }
            self.len = n;
        }
        Ok(())
    }

    fn read_exact_into(&mut self, out: &mut [u8]) -> io::Result<()> {
        let mut filled = 0;
        while filled < out.len() {
            self.fill()?;
            if self.pos == self.len {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated LZMA header",
                ));
            }
            let n = (self.len - self.pos).min(out.len() - filled);
            out[filled..filled + n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            filled += n;
        }
        Ok(())
    }
}

/// Streaming LZMA1 decoder over any [`Read`].
pub struct LzmaReader<R> {
    src: Source<R>,
    dec: LzmaDecoder,
    remaining: Option<u64>,
    done: bool,
}

impl<R: Read> LzmaReader<R> {
    /// Reads a 13-byte `.lzma` header from `inner` and prepares to decode the
    /// rest of the stream.
    ///
    /// # Errors
    ///
    /// Fails if the header is truncated or its properties are unsupported.
    pub fn new(inner: R) -> io::Result<Self> {
        let mut src = Source::new(inner);
        let mut header = [0u8; LZMA_ALONE_HEADER_SIZE];
        src.read_exact_into(&mut header)?;
        let header = LzmaAloneHeader::parse(&header).map_err(to_io)?;
        Ok(LzmaReader {
            src,
            dec: LzmaDecoder::new(header.props).map_err(to_io)?,
            remaining: header.uncompressed_size,
            done: false,
        })
    }

    /// Builds a reader over a raw LZMA1 stream with properties supplied out of
    /// band, as a 7z coder does.
    ///
    /// # Errors
    ///
    /// Fails if the decoder cannot be allocated.
    pub fn with_props(
        inner: R,
        props: LzmaProps,
        uncompressed_size: Option<u64>,
    ) -> io::Result<Self> {
        Ok(LzmaReader {
            src: Source::new(inner),
            dec: LzmaDecoder::new(props).map_err(to_io)?,
            remaining: uncompressed_size,
            done: false,
        })
    }
}

impl<R: Read> Read for LzmaReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.done || out.is_empty() {
            return Ok(0);
        }
        let mut written = 0usize;
        while written == 0 {
            self.src.fill()?;
            let input = &self.src.buf[self.src.pos..self.src.len];

            let limit = match self.remaining {
                Some(r) => (out.len() as u64).min(r) as usize,
                None => out.len(),
            };
            if limit == 0 {
                self.done = true;
                return Ok(0);
            }
            let finish = match self.remaining {
                Some(r) if (r as usize) <= limit => FinishMode::End,
                _ => FinishMode::Any,
            };

            let progress = self
                .dec
                .decode(input, &mut out[..limit], finish)
                .map_err(to_io)?;
            self.src.pos += progress.read;
            written = progress.written;
            if let Some(r) = self.remaining.as_mut() {
                *r -= written as u64;
                if *r == 0 {
                    self.done = true;
                }
            }

            match progress.status {
                Status::FinishedWithMark => self.done = true,
                Status::NeedsMoreInput if self.src.eof && progress.read == 0 && written == 0 => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "truncated LZMA stream",
                    ));
                }
                _ => {}
            }

            if written == 0 && self.done {
                break;
            }
            if written == 0 && progress.read == 0 && self.src.eof {
                self.done = true;
                break;
            }
        }
        Ok(written)
    }
}

/// Streaming LZMA2 decoder over any [`Read`].
pub struct Lzma2Reader<R> {
    src: Source<R>,
    dec: Lzma2Decoder,
    done: bool,
}

impl<R: Read> Lzma2Reader<R> {
    /// Builds a reader over a raw LZMA2 stream, given the single
    /// dictionary-size property byte that xz and 7z carry for it.
    ///
    /// # Errors
    ///
    /// Fails if the property byte is out of range or allocation fails.
    pub fn new(inner: R, dict_prop: u8) -> io::Result<Self> {
        Ok(Lzma2Reader {
            src: Source::new(inner),
            dec: Lzma2Decoder::new(dict_prop).map_err(to_io)?,
            done: false,
        })
    }
}

impl<R: Read> Read for Lzma2Reader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.done || out.is_empty() {
            return Ok(0);
        }
        let mut written = 0usize;
        while written == 0 {
            self.src.fill()?;
            let input = &self.src.buf[self.src.pos..self.src.len];
            let progress = self
                .dec
                .decode(input, out, FinishMode::Any)
                .map_err(to_io)?;
            self.src.pos += progress.read;
            written = progress.written;

            if progress.status == Status::FinishedWithMark {
                self.done = true;
            }
            if written == 0 {
                if self.done {
                    break;
                }
                if progress.read == 0 && self.src.eof {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "truncated LZMA2 stream",
                    ));
                }
            }
        }
        Ok(written)
    }
}
