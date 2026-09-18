# Porting the LZMA encoder to Rust

## Reference

The same checkout the decoder was ported from: github.com/ip7z/7zip, commit
`0766b733fe3e06dd2a7f9a3cfbf2108ac73abd17`, tag 26.03. All of it is public
domain (`C/` in the LZMA SDK).

| C file | Role | Rust target |
| --- | --- | --- |
| `C/LzFind.h` / `C/LzFind.c` | the window and the six match finders: `MatchFinder_Create`, `MatchFinder_Init`, `MatchFinder_CheckLimits`, `MatchFinder_SetLimits`, `MatchFinder_Normalize3`, `GetMatchesSpec1`, `SkipMatchesSpec`, `Hc_GetMatchesSpec`, and the `Bt2/Bt3/Bt4/Bt5/Hc4/Hc5` entry points | `src/enc/lz_find.rs` |
| `C/LzHash.h` | `HASH2_CALC`…`HASH5_CALC` and the `kLzHash_CrcShift_*` constants | `src/enc/consts.rs`, used in `src/enc/lz_find.rs` |
| `C/LzmaEnc.h` / `C/LzmaEnc.c` | `CLzmaEncProps` and `LzmaEncProps_Normalize`; the range encoder (`CRangeEnc`, `RangeEnc_ShiftLow`, `RC_BIT`, `RC_NORM`); the price tables (`ProbPrices`, `GET_PRICE*`, `LenPriceEnc_UpdateTables`, `FillDistancesPrices`, `FillAlignPrices`); the parser (`GetOptimum`, `GetOptimumFast`, `Backward`, `COptimal`); `LzmaEnc_CodeOneBlock`, `LzmaEnc_Encode`, `LzmaEnc_WriteProperties` | `src/enc/props.rs`, `src/enc/range_enc.rs`, `src/enc/price.rs`, `src/enc/lzma_enc.rs`, `src/enc/consts.rs` |
| `C/Lzma2Enc.h` / `C/Lzma2Enc.c` | the chunk layer: `Lzma2EncInt_InitBlock`, `Lzma2EncInt_EncodeSubblock` with its copy-chunk fallback, `CLimitedSeqInStream`, `Lzma2Enc_WriteProperties`, `LZMA2_DIC_SIZE_FROM_PROP` | `src/enc/lzma2_enc.rs`, `src/enc/stream.rs` |
| — | the `.xz` frame, from the format specification rather than from `C/XzEnc.c` | `src/enc/xz_enc.rs` |
| — | `std::io::Write` adapters, which the C has no equivalent of | `src/enc/write.rs` |

The rules in `AGENTS.md` and `docs/porting.md` apply here as they do to the
decoder: port, do not redesign; cite the C function name; `unsafe` only where
the C relies on a margin invariant.

## Features

The encoder is behind the `enc` feature, which is on by default and which
requires `crc`. It needs `crc` because the match finder's 256-entry byte table
is the standard reflected CRC-32 table, and `src/enc/lz_find.rs` derives it
from `crate::crc` rather than building it again from `kCrcPoly`: a one-shot
CRC-32 over the single byte `b` is `table[0xFF ^ b] ^ 0xFF000000`, so
`table[i] == crc32(&[i ^ 0xFF]) ^ 0xFF000000`. The unit test
`the_table_is_the_reflected_crc32_table` checks the derived table against the C
loop entry for entry. The hash *functions* over that table — `HASH2_CALC` and
friends, with their shift constants — are the SDK's own and are ported as they
stand; they are not a CRC of anything.

The `.xz` writer is behind `xz` as well, and its block checks come from the
same places the reader's do: CRC-32 and CRC-64/XZ from `crate::crc`
(`crc-fast`), SHA-256 from `crate::crypto` (`aws-lc-rs` behind `crypto`,
`sha2` behind `native-crypto`). A check this build cannot compute is a
parameter error rather than a stream written without it.

## What was left out

**`C/LzFindMt.c`.** The multi-threaded match finder is not ported. The port is
the shape the SDK takes when it is built with `-DZ7_ST`: `LzFind.c` alone,
driven from one thread.

**Multi-threaded `Lzma2Enc`.** `Lzma2Enc_Encode2`'s `MtCoder` path, which
splits the input into blocks and compresses them in parallel, is not ported.
`Lzma2Encoder` is the `Lzma2Enc_EncodeMt1` path with a solid block: one LZMA2
stream for whatever it is given.

**`directInput` mode.** `CMatchFinder` can be pointed at a caller's whole
buffer instead of copying into its own window; only the windowed path is here,
fed by the `SeqInStream` trait. The two modes differ in how much input is
visible at once, and not in the output: `lenLimit` is clamped to `matchMaxLen`
whenever more than `keepSizeAfter` bytes remain, `posLimit` only decides *when*
`MatchFinder_CheckLimits` runs, and normalization only happens on a `pos` wrap
past 2^32. `src/enc/stream.rs` sets this out at length.

**Two host-dependent constants, pinned to their 64-bit values.** Both derive
from `sizeof(size_t)` in the C, which would make one input at one setting
compress to different bytes on a 32-bit host:

- `kNumLogBits` in `LzmaEnc.c` is `(11 + sizeof(size_t) / 8 * 3)`, pinned to
  `11 + 3 = 14`;
- `LzmaEncProps_Normalize`'s default dictionary size is
  `level <= 4 ? 1 << (level * 2 + 16) : level <= sizeof(size_t) / 2 + 4 ? 1 << (level + 20) : 1 << 28`,
  pinned at the `sizeof(size_t) == 8` branch, so level 5 is `1 << 25`.

This crate is 64-bit-first, and a compressor whose output depends on the width
of the host's pointers is not one anybody wants.

**The progress callback.** `ICompressProgress` is not carried; there is no
caller of it in this crate.

## How the port is proved

**Bit-exact parity, `tests/lzma_parity.rs` and `tests/lzma2_encoder.rs`.**
`cargo xtask lzma-util` builds three binaries from the pinned SDK sources into
`target/lzma-util/`: `lzma`, which is `C/Util/Lzma/LzmaUtil.c` as it ships, and
`lzma-oracle` and `lzma2-oracle`, two small props-driven harnesses the xtask
writes out — `LzmaUtil` only ever encodes at `LzmaEncProps_Init` defaults and
never calls `LzmaEnc_SetDataSize`, which is too narrow an oracle. No reference
source is committed to this repository; the xtask reads it out of the pinned
commit. The tests compare this crate's bytes with the C's, byte for byte, over
a generated corpus — empty, one byte, all zeros, random, text-like, long
repeats, and inputs either side of the dictionary size and of the LZMA2 chunk
limits — across the match finders and several levels. They skip with a message
when the binaries are absent, and fail instead when
`LZMA_TURBO_LZMA_UTIL_REQUIRE` is set, as CI sets it.

Parity is what proves the compressed data. The `.xz` frame around it has no
such oracle, because the SDK's `XzEnc.c` is not what was ported, so it is
proved by decoding instead.

**Round trips, `tests/xz_encoder.rs`.** Every check type this build can write,
at several block sizes, over the same corpus, back through `XzReader`,
`XzParallelReader` and `XzAdaptiveDecoder` — the last one fed seven bytes at a
time for the small inputs. The `Write` adapters are checked to produce byte for
byte what the one-shot calls do.

**External decode, `tests/xz_encoder.rs`.** `xz -t` and `xz -dc` over this
crate's `.xz` output, and `xz -dc --format=lzma` over its `.lzma` output,
compared with the original bytes. `7zz t` too where it is installed. Both skip
with a message when the tool is not on `PATH`.

**Fuzzing, `fuzz/fuzz_targets/encode_round_trip.rs`.** Arbitrary bytes at
settings taken from the input: `.lzma` back through `LzmaReader`, raw LZMA2
back through `Lzma2Decoder`, and `.xz` back through `XzReader`.
