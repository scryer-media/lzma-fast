//! Arbitrary bytes through every decoder at once: lzma-turbo's assembly
//! loop, its portable loop and, when the fuzz build had `LZMA_SDK` set, the
//! LZMA SDK's C and assembly loops (see `tools/sdk-oracle`). They must agree
//! call for call — bytes read, bytes written, status or error — and on every
//! output byte. A crash here is a disagreement, not only a memory error.
#![no_main]

use libfuzzer_sys::fuzz_target;
use sdk_oracle::lockstep::{Shape, agree, decoders, trace_lzma, trace_lzma2};

/// Input slice sizes; the small ones keep the decoders in the `tempBuf`
/// single-symbol path.
const IN: [usize; 4] = [1, 7, 64, usize::MAX];
/// Output buffer sizes.
const OUT: [usize; 4] = [1, 13, 4096, 1 << 16];

fuzz_target!(|data: &[u8]| {
    let [mode, shape, data @ ..] = data else {
        return;
    };
    let shape = Shape {
        in_chunk: IN[usize::from(shape & 3)],
        out_chunk: OUT[usize::from((shape >> 2) & 3)],
        // A few kilobytes of input cannot legitimately need more than this.
        max_output: 1 << 24,
    };
    let decoders = decoders();
    let result = if mode & 1 == 0 {
        if data.len() < 13 {
            return;
        }
        // Keep the dictionary small: a fuzzer should not be able to ask for
        // 4 GiB. Past the cap the dictionary bytes are clamped, not skipped,
        // so the decoders still see the input.
        let mut stream = data.to_vec();
        let dict = u32::from_le_bytes([stream[1], stream[2], stream[3], stream[4]]);
        if dict > (1 << 24) {
            stream[1..5].copy_from_slice(&(1u32 << 24).to_le_bytes());
        }
        agree(&decoders, |w| trace_lzma(w, &stream, shape))
    } else {
        // Dictionary properties past 24 ask for more than 16 MiB; 41 and up are
        // invalid and must be refused by every decoder alike.
        let prop = match mode >> 1 {
            p @ 0..=24 => p,
            p if p > 40 => p,
            p => p % 25,
        };
        agree(&decoders, |w| trace_lzma2(w, prop, data, shape))
    };
    if let Err(e) = result {
        panic!("{e}");
    }
});
