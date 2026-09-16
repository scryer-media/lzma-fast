//! Shared test helpers.
//!
//! Includes a deliberately minimal `.xz` container reader: enough to find the
//! LZMA2 filter properties and the compressed data of a single-block stream so
//! the LZMA2 decoder can be driven from an `xz(1)` output. It is test-only;
//! the crate itself decodes raw LZMA/LZMA2 and leaves containers to callers.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use lzma_fast::{Error, FinishMode, Lzma2Decoder, LzmaAloneHeader, LzmaDecoder, Status};

pub fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

pub fn read(name: &str) -> Vec<u8> {
    std::fs::read(data_dir().join(name)).unwrap_or_else(|e| panic!("read {name}: {e}"))
}

/// Every committed source file and the stems of the streams built from it.
pub const SOURCES: &[&str] = &["text", "rand", "zeros", "mixed", "tiny", "empty"];
/// Suffixes of the committed LZMA1 (`.lzma`) vectors.
pub const LZMA_VARIANTS: &[&str] = &["p1", "p9e", "lc0lp2pb0", "lc4pb1"];
/// Suffixes of the committed LZMA2-in-xz vectors.
pub const XZ_VARIANTS: &[&str] = &["p1", "lc1lp1pb0"];

/// Outcome of a streaming decode: the bytes and the last status seen.
#[derive(Debug)]
pub struct Decoded {
    pub bytes: Vec<u8>,
    pub status: Status,
}

/// Drives [`LzmaDecoder::decode`] over a `.lzma` stream in fixed-size input
/// and output slices, which is what makes the `tempBuf` / `TryDummy` paths run.
pub fn decode_lzma(data: &[u8], in_chunk: usize, out_chunk: usize) -> Result<Decoded, Error> {
    decode_lzma_with(data, in_chunk, out_chunk, false)
}

/// As [`decode_lzma`], but `portable` forces the Rust port of the C fast loop
/// even on a target whose build has an assembly one.
pub fn decode_lzma_with(
    data: &[u8],
    in_chunk: usize,
    out_chunk: usize,
    portable: bool,
) -> Result<Decoded, Error> {
    if data.len() < 13 {
        return Err(Error::CorruptData);
    }
    let header: [u8; 13] = data[..13].try_into().expect("13 bytes");
    let header = LzmaAloneHeader::parse(&header)?;
    let mut dec = if portable {
        LzmaDecoder::new_portable(header.props)?
    } else {
        LzmaDecoder::new(header.props)?
    };

    let mut input = &data[13..];
    let mut out = Vec::new();
    let mut buf = vec![0u8; out_chunk.max(1)];
    let mut remaining = header.uncompressed_size;
    let mut status = Status::NotSpecified;

    loop {
        let feed = in_chunk.min(input.len());
        let limit = match remaining {
            Some(r) => buf.len().min(r as usize),
            None => buf.len(),
        };
        if limit == 0 {
            break;
        }
        let finish = match remaining {
            Some(r) if (r as usize) <= limit => FinishMode::End,
            _ => FinishMode::Any,
        };

        let p = dec.decode(&input[..feed], &mut buf[..limit], finish)?;
        status = p.status;
        input = &input[p.read..];
        out.extend_from_slice(&buf[..p.written]);
        if let Some(r) = remaining.as_mut() {
            *r -= p.written as u64;
            if *r == 0 {
                break;
            }
        }
        if status == Status::FinishedWithMark {
            break;
        }
        if p.read == 0 && p.written == 0 {
            break;
        }
    }

    Ok(Decoded { bytes: out, status })
}

/// Drives [`Lzma2Decoder::decode`] over a raw LZMA2 stream.
pub fn decode_lzma2(
    dict_prop: u8,
    data: &[u8],
    in_chunk: usize,
    out_chunk: usize,
) -> Result<Decoded, Error> {
    decode_lzma2_with(dict_prop, data, in_chunk, out_chunk, false)
}

/// As [`decode_lzma2`], but `portable` forces the Rust port of the C fast
/// loop.
pub fn decode_lzma2_with(
    dict_prop: u8,
    data: &[u8],
    in_chunk: usize,
    out_chunk: usize,
    portable: bool,
) -> Result<Decoded, Error> {
    let mut dec = if portable {
        Lzma2Decoder::new_portable(dict_prop)?
    } else {
        Lzma2Decoder::new(dict_prop)?
    };
    let mut input = data;
    let mut out = Vec::new();
    let mut buf = vec![0u8; out_chunk.max(1)];
    #[allow(unused_assignments)]
    let mut status = Status::NotSpecified;

    loop {
        let feed = in_chunk.min(input.len());
        let p = dec.decode(&input[..feed], &mut buf, FinishMode::Any)?;
        status = p.status;
        input = &input[p.read..];
        out.extend_from_slice(&buf[..p.written]);
        if status == Status::FinishedWithMark {
            break;
        }
        if p.read == 0 && p.written == 0 {
            break;
        }
    }

    Ok(Decoded { bytes: out, status })
}

/// The LZMA2 filter id in an xz block header.
const XZ_FILTER_LZMA2: u64 = 0x21;

/// Reads a multibyte integer as xz encodes them (7 bits per byte, high bit
/// continues).
fn xz_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut v: u64 = 0;
    for i in 0..9 {
        let b = *buf.get(*pos)?;
        *pos += 1;
        v |= u64::from(b & 0x7F) << (i * 7);
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// Minimal single-block `.xz` reader: returns the LZMA2 dictionary property
/// byte and the block's compressed data. Test-only, and deliberately strict —
/// it rejects anything it does not understand rather than guessing.
pub fn xz_lzma2_block(data: &[u8]) -> Option<(u8, &[u8])> {
    const MAGIC: &[u8] = &[0xFD, b'7', b'z', b'X', b'Z', 0x00];
    if data.len() < 12 || &data[..6] != MAGIC {
        return None;
    }
    let mut pos = 12; // stream header
    let first = *data.get(pos)?;
    if first == 0 {
        return None; // index, i.e. no block at all
    }
    let header_size = (usize::from(first) + 1) * 4;
    let header_end = pos + header_size;
    if header_end > data.len() {
        return None;
    }
    pos += 1;
    let flags = *data.get(pos)?;
    pos += 1;
    let num_filters = usize::from(flags & 0x03) + 1;
    if flags & 0x3C != 0 {
        return None; // reserved bits set
    }
    if flags & 0x40 != 0 {
        xz_varint(data, &mut pos)?; // compressed size
    }
    if flags & 0x80 != 0 {
        xz_varint(data, &mut pos)?; // uncompressed size
    }
    if num_filters != 1 {
        return None;
    }
    let id = xz_varint(data, &mut pos)?;
    let props_size = xz_varint(data, &mut pos)? as usize;
    if id != XZ_FILTER_LZMA2 || props_size != 1 {
        return None;
    }
    let dict_prop = *data.get(pos)?;
    Some((dict_prop, &data[header_end..]))
}

// ---------------------------------------------------------------------------
// Multi-run LZMA2 stream construction, for the multi-threaded decoder
// ---------------------------------------------------------------------------

/// Walks the chunk headers of a raw LZMA2 stream and returns the offset of the
/// `0x00` end marker, i.e. the length of the stream proper.
///
/// Header-only: it reads control bytes and sizes and never decodes. This is
/// the same walk the crate's run scanner does, kept independently here so a
/// test never asserts the implementation against itself.
pub fn lzma2_stream_len(data: &[u8]) -> Option<usize> {
    let mut pos = 0usize;
    loop {
        let control = *data.get(pos)?;
        if control == 0 {
            return Some(pos + 1);
        }
        if control < 3 {
            // Uncompressed chunk: 2-byte size-1, then the data.
            let size = usize::from(u16::from_be_bytes([
                *data.get(pos + 1)?,
                *data.get(pos + 2)?,
            ])) + 1;
            pos += 3 + size;
        } else if control < 0x80 {
            return None;
        } else {
            let pack = usize::from(u16::from_be_bytes([
                *data.get(pos + 3)?,
                *data.get(pos + 4)?,
            ])) + 1;
            let has_prop = (control >> 5) & 3 >= 2;
            pos += 5 + usize::from(has_prop) + pack;
        }
    }
}

/// One independently decodable LZMA2 run, and the bytes it decodes to.
pub struct Run {
    /// The run's chunks, without the stream's `0x00` end marker.
    pub packed: Vec<u8>,
    pub plain: Vec<u8>,
}

/// Reads a committed `.xz` fixture as a single LZMA2 run.
///
/// Every `xz(1)` block starts with a dictionary reset, so its chunks are a
/// complete run: strip the end marker and they concatenate with any other
/// run's chunks into a longer stream.
pub fn xz_run(name: &str) -> (u8, Run) {
    let raw = read(name);
    let (dict_prop, block) = xz_lzma2_block(&raw).expect("xz block");
    let len = lzma2_stream_len(block).expect("walk chunks");
    assert!(
        block[0] >= 0xE0 || block[0] == 0x01,
        "{name} does not start with a dict reset"
    );
    let plain = decode_lzma2(dict_prop, &block[..len], usize::MAX, 1 << 16)
        .expect("decode")
        .bytes;
    (
        dict_prop,
        Run {
            packed: block[..len - 1].to_vec(),
            plain,
        },
    )
}

/// Concatenates runs into one raw LZMA2 stream with a single end marker.
pub fn join_runs(runs: &[Run]) -> (Vec<u8>, Vec<u8>) {
    let mut packed = Vec::new();
    let mut plain = Vec::new();
    for r in runs {
        packed.extend_from_slice(&r.packed);
        plain.extend_from_slice(&r.plain);
    }
    packed.push(0);
    (packed, plain)
}

/// A stream of `repeat` copies of each named `.xz` fixture's run, in order.
///
/// Returns the dictionary property byte to decode it with (the largest of the
/// parts', which decodes them all), the packed stream and the expected output.
pub fn multi_run(names: &[&str], repeat: usize) -> (u8, Vec<u8>, Vec<u8>) {
    let mut prop = 0u8;
    let mut runs = Vec::new();
    for _ in 0..repeat {
        for name in names {
            let (p, r) = xz_run(name);
            prop = prop.max(p);
            runs.push(r);
        }
    }
    let (packed, plain) = join_runs(&runs);
    (prop, packed, plain)
}

/// Builds a run out of LZMA2 uncompressed chunks: a `0x01` chunk (copy, with a
/// dictionary reset) followed by `0x02` chunks (copy, no reset).
///
/// Useful where a test needs runs of a chosen size without an encoder, and
/// where the point is the framing rather than the LZMA decode.
pub fn copy_run(plain: &[u8]) -> Run {
    assert!(!plain.is_empty(), "an LZMA2 copy chunk cannot be empty");
    let mut packed = Vec::new();
    let mut first = true;
    let mut rest = plain;
    loop {
        let n = rest.len().min(1 << 16);
        packed.push(if first { 0x01 } else { 0x02 });
        let size = (n - 1) as u16;
        packed.extend_from_slice(&size.to_be_bytes());
        packed.extend_from_slice(&rest[..n]);
        first = false;
        rest = &rest[n..];
        if rest.is_empty() {
            break;
        }
    }
    Run {
        packed,
        plain: plain.to_vec(),
    }
}

/// A deterministic pseudo-random byte string, for fixtures a test builds.
pub fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
    let mut s = seed | 1;
    let mut v = Vec::with_capacity(len);
    for _ in 0..len {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        v.push((s >> 24) as u8);
    }
    v
}

/// The `.7z` pack-stream helper, shared with the benchmark harness so there is
/// one implementation of it and it lives outside the library.
#[path = "../../../../tools/lzma-bench/src/sevenz.rs"]
pub mod sevenz;
