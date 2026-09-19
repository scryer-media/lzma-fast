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
| `C/Lzma2Enc.h` / `C/Lzma2Enc.c` | the chunk layer: `Lzma2EncInt_InitBlock`, `Lzma2EncInt_EncodeSubblock` with its copy-chunk fallback, `CLimitedSeqInStream`, `Lzma2Enc_WriteProperties`, `LZMA2_DIC_SIZE_FROM_PROP`; and the block layer: `CLzma2EncInt` per block, `Lzma2Enc_EncodeMt1` in both its stream and its memory form, `Lzma2EncProps_Normalize`'s block-size and thread arithmetic, `Lzma2Enc_MtCallback_Code` / `_Write` | `src/enc/lzma2_enc.rs`, `src/enc/stream.rs` |
| `C/MtCoder.h` / `C/MtCoder.c` | `MtCoder_Code` and `ThreadFunc2`: the read token, the block semaphore, the free-block list and the in-order write turn | `src/enc/mt_coder.rs` |
| `C/Threads.h` / `C/Threads.c` | `CSemaphore` (`Semaphore_OptCreateInit`, `_Wait`, `_Release1`), beside the `CAutoResetEvent` the decoder already had | `src/mt/sync.rs`, `src/mt/event.rs` |
| `C/Bra.c` / `C/Bra86.c` / `C/BraIA64.c` | the branch converters in the encode direction: `z7_BranchConvSt_X86_Enc` and `z7_BranchConv_{ARM64,ARM,ARMT,PPC,SPARC,IA64,RISCV}_Enc` | `src/xz/bcj.rs`, beside the decode direction |
| `C/Delta.c` | `Delta_Encode` | `src/xz/delta.rs`, beside `Delta_Decode` |
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

## The filter encoders

The `.xz` writer can put a filter chain in front of LZMA2 — up to three of the
delta and BCJ converters, which is what the format allows — and
`src/enc/xz_enc.rs` writes the matching filter flags into each block header, in
the order the filters were applied, LZMA2 last.

The converters themselves live beside their decode halves. In the C, both
directions are one function taking an `encoding` flag, with
`BR_CONVERT_VAL(v, c)` expanding to `v += c` or `v -= c`; this port keeps that
shape — one `*_conv` per converter taking the flag, with `*_decode` and
`*_encode` wrappers built by the same kind of macro the C uses. Two converters
are not a flag apart and the C says so itself:

- **IA64** masks its running `pc` differently in each direction
  (`pc &= (0x1fffff << 1) | 1` encoding, `pc |= ~((0x1fffff << 1) | 1)`
  decoding), and mutates it in place, so the mutation persists across
  iterations. The port does the same.
- **RISC-V** is written out as two whole functions in `Bra.c`, because the JAL
  arm rebuilds the instruction from different halves in each direction and the
  AUIPC arm that converts is the other one. `riscv_encode` is the port of
  `Z7_BRANCH_CONV_ENC(RISCV)`; it is not `riscv_decode` with a sign flipped.

`Bcj::encode` keeps the same carry contract as `Bcj::decode`: it converts a
prefix and leaves the tail of a straddling instruction for the next call, so
encoding a buffer in pieces gives what encoding it whole gives. That is what
lets the writer filter a whole block in one call while the readers undo it
chunk by chunk.

## What was left out

**`C/LzFindMt.c` and `C/LzFindOpt.c`.** The *threaded match finder* is still
not ported — `MatchFinderMt_*`, the hash thread / bt thread pipeline with its
two ring buffers, and `GetMatchesSpecN_2`. Every match finder here is the one
`LzFind.c` runs on the calling thread, which is what the SDK uses when
`lzmaProps.numThreads` is 1. This costs nothing in output: the SDK's `btMode`
MT finder is built to produce the same matches as the ST one, so the bytes are
the same either way; it costs the second and third core that the C can put
behind a *single* block. The parallelism that is here is block-parallelism,
described below, which is the parallelism `xz -T` and 7-Zip's `mt` actually
ship with.

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

## The multi-threaded encoder

LZMA2 has exactly one seam a stream can be cut along: a *block*, a run of
chunks that opens by resetting the dictionary and therefore decodes without
reference to anything before it. `C/Lzma2Enc.c` splits the input at that seam
and hands the pieces to `C/MtCoder.c`; that is what is ported here.

| C | Rust |
| --- | --- |
| `CMtCoder`, `MtCoder_Code` | `MtCoder::code` in `src/enc/mt_coder.rs` |
| `ThreadFunc2`, the `readEvent` token passed thread to thread | `Run::thread_func2`, `Run::read_event` |
| `blocksSemaphore`, `freeBlockList`, `ReadyBlocks[]`, `writeIndex` | `Run::blocks_semaphore`, `Shared::free_block_list`, `Shared::ready_blocks`, `Shared::write_index` |
| `MTCODER_GET_NUM_BLOCKS_FROM_THREADS`, `MTCODER_BLOCKS_MAX`, the `+1` per block-size band | `num_blocks_from_threads`, `BLOCKS_MAX`, `Run::num_blocks_max` |
| `MtProgress_GetError` / `_SetError` | `Run::get_error` / `Run::set_error` |
| `SeqInStream_ReadMax` | `read_max` |
| `IMtCoderCallback2` (`Code`, `Write`) | the `MtCoderCallback` trait |
| `CLzma2EncInt`, `Lzma2EncInt_InitBlock` | `Lzma2EncInt`, `Lzma2EncInt::init_block` |
| `Lzma2Enc_EncodeMt1`, `inStream` form | `Lzma2EncInt::encode_mt1_stream` |
| `Lzma2Enc_EncodeMt1`, `inData` form | `Lzma2EncInt::encode_mt1_mem` |
| `Lzma2Enc_MtCallback_Code` / `_Write`, `me->coders[]`, `me->outBufs[]` | `Lzma2MtCallback` |
| `Lzma2EncProps_Normalize`: `blockSize` AUTO/SOLID, the `reduceSize` override, `numBlockThreads_Reduced` | `auto_block_size`, `Lzma2Encoder::block_size`, `::effective_props`, `::threads_reduced` |
| the `memUsage` reduction 7-Zip applies outside `C/` | `Lzma2Encoder::set_mem_limit`, `::mem_usage_per_thread` |
| `MatchFinder_Create`'s sizing, split out so it can be costed without allocating | `MatchFinder::plan`, `MatchFinder::mem_usage`, `LzmaEnc::mem_usage` |

`XzEncoder` needs none of `MtCoder`: `.xz` blocks are independent by
construction and the writer already queues one at a time, so
`XzEncoder::set_threads` compresses the queued blocks in a `std::thread::scope`
and appends them in order. Filter chains come along unchanged — the format
resets filter state at every block boundary, so each block builds its own
converters exactly as the single-threaded path does.

**The bytes do not depend on the thread count.** For one `(props, block size,
data size)` this encoder produces one stream, at one thread and at sixteen.
That is not an accident of scheduling: a block is encoded from its own bytes
alone, and the only thing the thread count touches is which core ran it.
`Lzma2Enc_Encode2` is the same. It is checked three ways — a unit test, a
property in the fuzz target, and `tests/lzma2_mt_parity.rs` against the C.

### What is deliberately different

- **Threads live for one call.** The C parks its worker threads on a
  `startEvent` and reuses them across files; `MtCoder::code` runs its workers
  inside a `std::thread::scope`, which is what lets them borrow the caller's
  input and callback instead of taking ownership. The thread count, the block
  scheduling and the output are the same; only the pool's lifetime differs.
- **`MTCODER_USE_WRITE_THREAD` is not carried.** The C `#undef`s it too, so
  this is the path the C actually takes.
- **`ICompressProgress` is not carried**, as elsewhere in this port.
- **`numThreadsMax` is the reduced count.** The C switches to the `MtCoder`
  path on `numBlockThreads_Reduced` but passes `numBlockThreads_Max` to
  `MtCoder`; this port passes the reduced count to both, because the reduced
  count is the one the memory budget and the block count allow. The output does
  not depend on it.
- **Output buffers grow.** The C sizes each block's output buffer at
  `blockSize + (blockSize >> 10) + 16` and fails with `SZ_ERROR_OUTPUT_EOF` if
  a block does not fit; here they are `Vec`s, so the copy-chunk fallback's
  worst case cannot overflow one.
- **The memory estimate is this port's own.** 7-Zip computes `GetMemUsage`
  outside `C/`, in LGPL C++ that must not be copied into this crate, so
  `mem_usage_per_thread` adds up this port's own allocation sites instead: the
  match finder's window and reference tables from `MatchFinder::plan`, the
  literal probability arrays, and the block's buffers.

### The threaded stream path

`Lzma2Encoder::encode_mt` runs `MtCoder` over a `SeqInStream` rather than a
slice, as `Lzma2Enc_Encode2` does when it is given one. It does not in general
produce what the single-threaded `encode` produces for the same input, and the
C has the same gap: a block read into memory is encoded knowing its own length,
whereas the single-threaded loop tells the encoder the block size until the
stream runs out, and `expectedDataSize` sizes the match finder's hash table.
Tell the encoder the real length with `set_data_size` and the two agree, which
is what `encode_to_vec` does.

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

**Threaded parity, `tests/lzma2_mt_parity.rs`.** `cargo xtask lzma-util` also
builds `lzma2-oracle-mt`: the same props-driven LZMA2 harness compiled
*without* `-DZ7_ST`, so `MtCoder.c`, `LzFindMt.c`, `LzFindOpt.c` and `Threads.c`
are in it and `Lzma2Enc_Encode2` takes its `MtCoder` path. It takes a block
size, a block thread count and a match-finder thread count on the command line.
The test compares this crate against it over the whole corpus at four block
sizes — 16 KiB, 64 KiB, 100000 and 1 MiB, either side of the corpus's own sizes
— and at one, two and four block threads; then checks that the bytes are the
same at one, two, three, eight and seventeen threads; then decodes them back
through `Lzma2Reader`. CI runs it in the `encoder-parity` job on all four
platforms.

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

**Filter parity, `tests/filter_parity.rs`.** `cargo xtask lzma-util` also
builds `filter-oracle` from the SDK's own `Bra.c`, `Bra86.c`, `BraIA64.c` and
`Delta.c`. Every BCJ kind and several delta distances are compared with it byte
for byte, in *both* directions, at several start offsets, over the shared
corpus plus generated code-like inputs — random bytes rarely contain the
instructions a branch converter looks for, so the corpus plants each kind's
opcode at its alignment. The same test checks that feeding a converter one,
three, seven, sixteen or 4096 bytes at a time gives what feeding it the whole
buffer gives.

**Filtered round trips and external decode, `tests/xz_encoder.rs`.** Every BCJ
kind on its own, with and without a start offset, three delta distances and a
delta-then-BCJ chain, at two block sizes, back through all three readers; and
`xz -t` and `xz -dc` over one large input per chain. Chains the format forbids
— LZMA2 as a non-last filter, a misaligned BCJ start offset, four non-last
filters — are checked to be refused.

**Fuzzing, `fuzz/fuzz_targets/encode_round_trip.rs`.** Arbitrary bytes at
settings taken from the input: `.lzma` back through `LzmaReader`, raw LZMA2
back through `Lzma2Decoder`, and `.xz` back through `XzReader` — then the same
`.xz` through a filter chain the input picks, back through all three readers,
the adaptive one fed in chunks the input sizes. The same input is also encoded
at a block size and thread count the input picks, both as raw LZMA2 and as
filtered `.xz`, and asserted to be byte for byte what one thread produces at
that block size.
