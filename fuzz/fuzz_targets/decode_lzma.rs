//! Arbitrary bytes as a `.lzma` stream. The decoder must return an error or
//! ask for more input; it must never panic and never read out of bounds.
#![no_main]

use libfuzzer_sys::fuzz_target;
use lzma_turbo::{FinishMode, LzmaAloneHeader, LzmaDecoder, Status};

fuzz_target!(|data: &[u8]| {
    if data.len() < 14 {
        return;
    }
    // The first byte picks the streaming shape so the tempBuf / TryDummy paths
    // are fuzzed too, not just the whole-buffer one.
    let in_chunk = match data[0] % 4 {
        0 => 1usize,
        1 => 7,
        2 => 64,
        _ => usize::MAX,
    };
    let data = &data[1..];

    let header: [u8; 13] = data[..13].try_into().unwrap();
    let Ok(header) = LzmaAloneHeader::parse(&header) else {
        return;
    };
    // Keep the dictionary small: a fuzzer should not be able to ask for 4 GiB.
    if header.props.dict_size() > (1 << 24) {
        return;
    }
    let Ok(mut dec) = LzmaDecoder::new(header.props) else {
        return;
    };

    let mut input = &data[13..];
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
        // A few kilobytes of input cannot legitimately produce gigabytes here.
        if produced > (1 << 28) {
            return;
        }
    }
});
