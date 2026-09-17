//! Arbitrary bytes as a raw LZMA2 stream, with the dictionary property byte
//! taken from the input.
#![no_main]

use libfuzzer_sys::fuzz_target;
use lzma_fast::{FinishMode, Lzma2Decoder, Status};

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }
    let in_chunk = match data[0] % 4 {
        0 => 1usize,
        1 => 7,
        2 => 64,
        _ => usize::MAX,
    };
    // Cap the dictionary so the fuzzer cannot drive a multi-gigabyte alloc.
    let dict_prop = data[1] % 27;
    let Ok(mut dec) = Lzma2Decoder::new(dict_prop) else {
        return;
    };

    let mut input = &data[2..];
    let mut out = vec![0u8; 4096];
    let mut produced: u64 = 0;
    loop {
        let feed = in_chunk.min(input.len());
        let Ok(p) = dec.decode(&input[..feed], &mut out, FinishMode::Any) else {
            return;
        };
        input = &input[p.read..];
        produced += p.written as u64;
        if p.status == Status::FinishedWithMark || (p.read == 0 && p.written == 0) {
            return;
        }
        if produced > (1 << 28) {
            return;
        }
    }
});
