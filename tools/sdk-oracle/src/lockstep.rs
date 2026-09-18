//! Driving lzma-turbo and the SDK through the same calls.
//!
//! `LzmaDecoder::decode` and `Lzma2Decoder::decode` port
//! `LzmaDec_DecodeToBuf` and `Lzma2Dec_DecodeToBuf`, so given the same input
//! and output slices every call must read and write the same number of bytes
//! and stop with the same status or fail with the same error. A [`Trace`]
//! records all of that for one decoder and one stream; [`agree`] runs every
//! decoder over a stream and names the first difference.

use lzma_turbo::{Error, FinishMode, Lzma2Decoder, LzmaDecoder, LzmaProps, Status};

use crate::{
    Code, Decoder, SZ_ERROR_DATA, SZ_ERROR_FAIL, SZ_ERROR_MEM, SZ_ERROR_UNSUPPORTED, Step,
    VARIANTS, Variant,
};

/// One of the decoders compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    /// lzma-turbo as built: its assembly loop where it has one.
    Ours,
    /// lzma-turbo's portable Rust loop.
    OursPortable,
    /// The SDK.
    Sdk(Variant),
}

/// The crate's two loops, then every SDK build this crate has.
#[must_use]
pub fn decoders() -> Vec<Which> {
    let mut all = vec![Which::Ours, Which::OursPortable];
    all.extend(VARIANTS.iter().map(|&v| Which::Sdk(v)));
    all
}

/// How the streams are fed.
#[derive(Debug, Clone, Copy)]
pub struct Shape {
    /// Input bytes offered per call.
    pub in_chunk: usize,
    /// Output buffer per call.
    pub out_chunk: usize,
    /// Stop once this many bytes have been produced.
    pub max_output: usize,
}

impl Shape {
    #[must_use]
    pub const fn new(in_chunk: usize, out_chunk: usize) -> Self {
        Shape {
            in_chunk,
            out_chunk,
            max_output: usize::MAX,
        }
    }
}

/// Everything a decoder did with one stream.
#[derive(Debug, PartialEq, Eq)]
pub struct Trace {
    /// The constructor's error, if it failed.
    pub open: Option<Code>,
    pub calls: Vec<Result<Step, Code>>,
    pub output: Vec<u8>,
}

fn code(error: Error) -> Code {
    match error {
        Error::CorruptData => SZ_ERROR_DATA,
        Error::UnsupportedProps => SZ_ERROR_UNSUPPORTED,
        Error::InternalFailure => SZ_ERROR_FAIL,
        Error::Alloc => SZ_ERROR_MEM,
        other => panic!("the single-threaded decoder returned {other:?}"),
    }
}

fn status(status: Status) -> u8 {
    match status {
        Status::NotSpecified => 0,
        Status::FinishedWithMark => 1,
        Status::NotFinished => 2,
        Status::NeedsMoreInput => 3,
        Status::MaybeFinishedWithoutMark => 4,
        other => panic!("a status the SDK has no value for: {other:?}"),
    }
}

const FINISHED_WITH_MARK: u8 = 1;

enum Any {
    Lzma(LzmaDecoder),
    Lzma2(Lzma2Decoder),
    Sdk(Decoder),
}

impl Any {
    fn lzma(which: Which, props: &[u8; 5]) -> Result<Self, Code> {
        match which {
            Which::Sdk(v) => Decoder::lzma(v, props).map(Any::Sdk),
            _ => {
                let props = LzmaProps::parse(props).map_err(code)?;
                let decoder = if which == Which::OursPortable {
                    LzmaDecoder::new_portable(props)
                } else {
                    LzmaDecoder::new(props)
                };
                decoder.map(Any::Lzma).map_err(code)
            }
        }
    }

    fn lzma2(which: Which, dict_prop: u8) -> Result<Self, Code> {
        match which {
            Which::Sdk(v) => Decoder::lzma2(v, dict_prop).map(Any::Sdk),
            Which::OursPortable => Lzma2Decoder::new_portable(dict_prop)
                .map(Any::Lzma2)
                .map_err(code),
            Which::Ours => Lzma2Decoder::new(dict_prop).map(Any::Lzma2).map_err(code),
        }
    }

    fn decode(&mut self, input: &[u8], output: &mut [u8], end: bool) -> Result<Step, Code> {
        let finish = if end {
            FinishMode::End
        } else {
            FinishMode::Any
        };
        let progress = match self {
            Any::Sdk(d) => return d.decode(input, output, end),
            Any::Lzma(d) => d.decode(input, output, finish),
            Any::Lzma2(d) => d.decode(input, output, finish),
        }
        .map_err(code)?;
        Ok(Step {
            read: progress.read,
            written: progress.written,
            status: status(progress.status),
        })
    }
}

/// A `.lzma` stream (13-byte header, then data), driven as the crate's
/// `tests/common::decode_lzma_with` drives it: `LZMA_FINISH_END` for the call
/// whose output reaches the size in the header.
///
/// # Panics
///
/// If `stream` is shorter than the header.
#[must_use]
pub fn trace_lzma(which: Which, stream: &[u8], shape: Shape) -> Trace {
    let mut trace = Trace {
        open: None,
        calls: Vec::new(),
        output: Vec::new(),
    };
    let props: [u8; 5] = stream[..5].try_into().expect("5 bytes");
    let size = u64::from_le_bytes(stream[5..13].try_into().expect("8 bytes"));
    let mut remaining = (size != u64::MAX).then_some(size);
    let mut decoder = match Any::lzma(which, &props) {
        Ok(d) => d,
        Err(e) => {
            trace.open = Some(e);
            return trace;
        }
    };
    let mut input = &stream[13..];
    let mut buf = vec![0u8; shape.out_chunk];
    while trace.output.len() < shape.max_output {
        let limit = remaining.map_or(buf.len(), |r| buf.len().min(r as usize));
        if limit == 0 {
            break;
        }
        let end = remaining.is_some_and(|r| r as usize <= limit);
        let feed = shape.in_chunk.min(input.len());
        let step = decoder.decode(&input[..feed], &mut buf[..limit], end);
        trace.calls.push(step);
        let Ok(step) = step else { break };
        input = &input[step.read..];
        trace.output.extend_from_slice(&buf[..step.written]);
        if let Some(r) = remaining.as_mut() {
            *r -= step.written as u64;
        }
        if step.status == FINISHED_WITH_MARK || (step.read == 0 && step.written == 0) {
            break;
        }
    }
    trace
}

/// A raw LZMA2 stream, `LZMA_FINISH_ANY` throughout.
#[must_use]
pub fn trace_lzma2(which: Which, dict_prop: u8, stream: &[u8], shape: Shape) -> Trace {
    let mut trace = Trace {
        open: None,
        calls: Vec::new(),
        output: Vec::new(),
    };
    let mut decoder = match Any::lzma2(which, dict_prop) {
        Ok(d) => d,
        Err(e) => {
            trace.open = Some(e);
            return trace;
        }
    };
    let mut input = stream;
    let mut buf = vec![0u8; shape.out_chunk];
    while trace.output.len() < shape.max_output {
        let feed = shape.in_chunk.min(input.len());
        let step = decoder.decode(&input[..feed], &mut buf, false);
        trace.calls.push(step);
        let Ok(step) = step else { break };
        input = &input[step.read..];
        trace.output.extend_from_slice(&buf[..step.written]);
        if step.status == FINISHED_WITH_MARK || (step.read == 0 && step.written == 0) {
            break;
        }
    }
    trace
}

/// Runs `run` for every decoder and compares each trace with the first.
///
/// # Errors
///
/// The first decoder that disagrees, and where.
pub fn agree(decoders: &[Which], run: impl Fn(Which) -> Trace) -> Result<(), String> {
    let reference = run(decoders[0]);
    for &which in &decoders[1..] {
        let trace = run(which);
        if trace == reference {
            continue;
        }
        let detail = if trace.open != reference.open {
            format!("open: {:?} vs {:?}", reference.open, trace.open)
        } else if let Some(i) = (0..reference.calls.len().max(trace.calls.len()))
            .find(|&i| reference.calls.get(i) != trace.calls.get(i))
        {
            format!(
                "call {i}: {:?} vs {:?}",
                reference.calls.get(i),
                trace.calls.get(i)
            )
        } else {
            let i = (0..reference.output.len().max(trace.output.len()))
                .find(|&i| reference.output.get(i) != trace.output.get(i))
                .unwrap_or(0);
            format!("output byte {i}")
        };
        return Err(format!(
            "{:?} and {which:?} disagree at {detail}",
            decoders[0]
        ));
    }
    Ok(())
}
