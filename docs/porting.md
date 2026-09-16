# Porting LzmaDec.c to Rust

## Reference

- Source tree: `~/dev/supporting-codebases/7zip` (github.com/ip7z/7zip),
  commit `0766b733fe3e06dd2a7f9a3cfbf2108ac73abd17`, tag 26.03.
- Files, all public domain (`C/` and `Asm/` in the LZMA SDK):

| C file | Lines | Role | Rust target |
| --- | --- | --- | --- |
| `C/LzmaDec.h` | – | state struct, props, API | `src/lzma/state.rs` |
| `C/LzmaDec.c` | 1367 | `LzmaDec_DecodeReal` (fast loop), `LzmaDec_TryDummy` (careful path), `LzmaDec_DecodeToDic`, `LzmaDec_DecodeToBuf` | `src/lzma/decode.rs`, `src/lzma/dummy.rs`, `src/lzma/mod.rs` |
| `C/Lzma2Dec.h` / `C/Lzma2Dec.c` | 493 | LZMA2 chunk framing, prop/dict resets | `src/lzma2/mod.rs` |
| `C/7zTypes.h`, `C/Compiler.h`, `C/Precomp.h` | – | macros (`kNumBitModelTotalBits`, `Z7_...`) | constants in `src/lzma/consts.rs` |
| `C/Util/Lzma/LzmaUtil.c` | – | `.lzma` header parse, streaming loop | `tools/lzma-bench` and tests |
| `C/MtDec.h` / `C/MtDec.c` | 1116 | the generic ring of worker threads: `MtDec_ThreadFunc2`, the `canRead`/`canWrite` token pair, `MtDec_Read` replay, `MTDEC_PARSE_*` | `src/mt/mtdec.rs`, `src/mt/event.rs` |
| `C/Lzma2DecMt.h` / `C/Lzma2DecMt.c` | 1000 | `CLzma2DecMtThread`, the four `Lzma2DecMt_MtCallback_*` callbacks, `Lzma2Dec_Decode_ST` | `src/mt/lzma2.rs`, `src/mt/mod.rs` |
| `C/Lzma2Dec.c` (`Lzma2Dec_Parse`) | – | walks chunk headers to find block boundaries | `src/lzma2/parse.rs`, `src/lzma2/frame.rs` |

`CPP/7zip/Compress/Lzma2Decoder.cpp` is LGPL, not public domain. It was read
only to see how `7zz` drives `Lzma2DecMt` — the block-size heuristic
(`Get_ExpectedBlockSize_From_Dict`, `kOverheadSize`, the memory-limited thread
count) is reimplemented from its behaviour in `src/mt/mod.rs`, not copied.

Not in scope: `XzDec.c`, `7zDec.c`, and the `Asm/x86/LzmaDecOpt.asm` /
`Asm/arm64/LzmaDecOpt.S` hand-written loops. The C fast loop compiled with the toolchain defaults is
the parity target for the port; the asm loop is what `7zz` ships and is the
acceptance target for the optimization phase.

## Rules

1. Faithful first. Port `LzmaDec_DecodeReal` as one function with the same
   locals (`range`, `code`, `buf`, `probs`, `dic`, `dicPos`, `processedPos`,
   `checkDicSize`, `state`, `rep0..rep3`, `limit`, `len`), the same macro
   expansions (`NORMALIZE`, `IF_BIT_0`, `UPDATE_0`, `UPDATE_1`, `TREE_DECODE`,
   `MATCHED_LITER_DEC`, `REV_BIT`), the same probability table layout and
   offsets, the same state transitions. Keep the C comments. Name things as
   the C does, in snake_case.
2. Same shape, same guarantees. `DecodeReal` runs only while
   `dicPos < limit` and `buf` has at least `LZMA_REQUIRED_INPUT_MAX` (20)
   bytes of margin. The wrapper (`LzmaDec_DecodeToDic`) enforces that and
   routes the tail through `LzmaDec_TryDummy` exactly as the C does. That
   margin is what makes unchecked loads sound; document it as the invariant
   on every `unsafe` block.
3. Unchecked where the C is unchecked, and nowhere else. Raw pointer or
   `get_unchecked` access is allowed inside the fast loop for `buf`, `probs`
   and `dic` under the margin invariant. Everything outside the loop is
   checked Rust.
4. Port, then measure, then optimize. Do not restructure before a faithful
   port passes differential tests and has a baseline number. Optimization
   means codegen: checking the assembly for bounds checks and spills, not
   redesigning the loop.
5. No `#[inline(never)]` splits of the hot loop, no trait-object dispatch in
   it, no per-bit function calls that the compiler may not inline.
6. Every step is checked against the oracle. See `docs/benchmarking.md`.

## Public API (initial)

```rust
pub struct LzmaProps { lc: u8, lp: u8, pb: u8, dict_size: u32 }
impl LzmaProps { pub fn parse(props: &[u8; 5]) -> Result<Self, Error>; }

pub struct LzmaDecoder { /* LzmaDec state + dictionary */ }
impl LzmaDecoder {
    pub fn new(props: LzmaProps) -> Result<Self, Error>;
    /// One call of LzmaDec_DecodeToBuf: consumes from `input`, writes to
    /// `output`, returns bytes read, bytes written and the status.
    pub fn decode(&mut self, input: &[u8], output: &mut [u8], finish: FinishMode)
        -> Result<Progress, Error>;
    pub fn reset(&mut self);
}

pub struct Lzma2Decoder { /* Lzma2Dec state over an LzmaDecoder */ }
impl Lzma2Decoder {
    pub fn new(dict_prop: u8) -> Result<Self, Error>;
    pub fn decode(&mut self, input: &[u8], output: &mut [u8], finish: FinishMode)
        -> Result<Progress, Error>;
}

pub enum Status { NotFinished, FinishedWithMark, NeedsMoreInput, MaybeFinishedWithoutMark }
pub enum Error { UnsupportedProps, CorruptData, /* … */ }
```

`std::io::Read` adapters (`LzmaReader`, `Lzma2Reader`) sit on top of that in
the `std` feature and are what a container parser consumes.

## Acceptance gate

Single-threaded decode wall time on `bench/fixtures/*.lzma` and the
`st.7z` pack stream, median of three, within 3% of `7zz t -mmt=1` on the
same file on the same idle machine. Until the port is at parity with the C
fast loop (`7lzma d`, no asm), the 3% number against `7zz` is not the
question; get to C parity first, then close on the asm.

## The container layer

The decoder is deliberately format-free: it decodes raw LZMA1 and raw LZMA2
and knows nothing about the files those streams arrive in. The layer above it
is not written yet; this section fixes its shape so that when it is, it is
ported from the same tree with the same discipline.

The container this crate covers is xz, and only xz. 7z — its header, folders
and coder graphs, its BCJ and delta filters, its AES-256 and the `7zAes` key
derivation — is out of scope here and is handled by a fork of `sevenz-rust2`
that depends on this crate.

Two things the xz layer needs first, and which the crate already has: the
checksums in [`crate::crc`] (`crc` feature, `crc-fast`; CRC-32 is xz check
type 1 and CRC-64/XZ is type 4) and the SHA-256 in [`crate::crypto`]
(check type 10), which is `aws-lc-rs` under the `crypto` feature and
RustCrypto's `sha2` under `native-crypto`, the latter winning when both are
on. All three are on by default, because every xz stream carries one of the
three checks; all three are unreachable from the decoder, and
`--no-default-features` has none of them.

### xz

C: `C/Xz.h`, `C/XzIn.c`, `C/XzDec.c`.

A stream is a 12-byte header (magic `FD 37 7A 58 5A 00`, then two flag bytes
whose low nibble is the check type and whose CRC-32 covers the pair), a series
of blocks, an index, and a 12-byte footer. A block is a header naming one to
four filters — for this crate the only interesting chain is a single LZMA2
filter, id `0x21`, whose one property byte is the dictionary size — followed
by the filter output, padding to a multiple of four, and the check. The check
is CRC-32, CRC-64/XZ or SHA-256 depending on the stream flags, or absent.
`tests/common/mod.rs` already contains the minimum of this reader, in the
strictest possible form: it rejects everything it does not understand rather
than guessing, and the real one should keep that habit.

The index is what makes random access possible: it records, for every block,
the compressed and uncompressed size, so a reader can seek to a block boundary
without decoding what precedes it. The footer repeats the index size and its
CRC-32 so the index can be found from the end of the file.

None of this is implemented. When it is: the same rules as the decoder — port
rather than redesign, cite the C function, prove it differentially against
`7zz x` and `xz -dc`, and keep it behind features so that a caller who only
wants raw LZMA still gets a crate with no dependencies.


## Adaptive use

The multi-threaded decoder has a second consumer besides "decode this archive
as fast as possible": a caller decoding an archive *while it downloads*. Such
a caller stays single-threaded while it is chasing the tail of the byte flow —
low latency, low memory, decode whatever has arrived — and spreads across
threads only once a backlog of fully-arrived runs has built up, then goes back
when the backlog drains. It does this mid-stream, repeatedly.

`Lzma2DecMt`'s structure cannot serve that. In the C the threads *are* the
control flow: `MtDec_ThreadFunc2` hands two auto-reset event tokens around a
ring, thread 0 runs on the caller's stack, input is pulled from an
`ISeqInStream` that is expected to block, and the whole ring exists for the
duration of one `Lzma2DecMt_Decode` call. There is no point at which a caller
can be asked what it would like to do next.

So the port keeps the ring as the throughput path and adds a second driver
beside it. Both decode the same runs, found by the same header walk, so they
produce the same bytes:

| Path | Rust | Shape |
| --- | --- | --- |
| Throughput | `Lzma2ParallelDecoder`, `Lzma2ParallelReader` | faithful `MtDec` ring; pull from a `Read`; blocks for the whole call |
| Adaptive | `Lzma2AdaptiveDecoder` | fed input, polled output, mode switched mid-stream |
| Boundaries | `Lzma2RunScanner` | shared by both, and public |

How each constraint is met:

1. **Push/feed input, not only pull.** `Lzma2AdaptiveDecoder::feed` copies
   bytes in and returns immediately; it never decodes and never blocks.
   `drain` hands decoded blocks to a sink and returns `NeedsMoreInput`,
   `Progress` or `Finished`. Chunk headers split across feeds are carried in
   `Lzma2Frame`, which is O(1) state: nothing is ever re-scanned.
   (`tests/adaptive.rs::feeding_one_byte_at_a_time_decodes_the_same_stream`.)

2. **Run discovery is a separate, cheap, public function over bytes.**
   `Lzma2RunScanner` reads control bytes and skips payloads — O(chunks), no
   decoding — and yields `Lzma2Run { in_offset, packed_len, out_offset,
   unpacked_len, has_dict_reset }` as runs complete. `pending_runs` and
   `backlog` on the decoder are that index, so the caller sees exactly what
   the decoder sees. (`tests/scan.rs`, and
   `tests/adaptive.rs::the_backlog_of_complete_runs_is_visible`.)

   Deviation: the throughput path still uses `Lzma2Dec_Parse`
   (`src/lzma2/parse.rs`) rather than the scanner, because that parse is
   entangled with `MtDec`'s block sizing and replacing it would stop the ring
   being a port. The two cannot disagree about where a run starts: both run
   `Lzma2Frame`, the single copy of the chunk-header state machine.

3. **Mode switching at run boundaries is lossless and cheap.**
   `set_threads(n)` takes effect at the next dispatch decision, which is only
   ever made at a run boundary. A run begins with a dictionary reset, so a
   chase decoder finishing one run and a worker starting the next are
   equivalent by construction; no input is re-fed and nothing is re-decoded.
   `threads == 1` dispatches nothing, wakes nobody, and writes to the sink
   straight out of the decoder's dictionary.
   (`tests/adaptive.rs::switching_from_st_to_mt_at_any_run_boundary_is_lossless`
   covers every N; `switching_back_to_single_threaded_mid_stream_is_lossless`
   flaps in both directions.)

4. **Output is (offset, bytes), in order by default.** Every block carries its
   unpacked offset. `set_ordered(false)` releases blocks as they are decoded,
   for a sink that writes by offset. The `Read` adapter and the push `decode`
   keep in-order delivery.
   (`tests/adaptive.rs::ordered_delivery_is_the_default_and_unordered_is_opt_in`.)

5. **Workers idle cheaply and outlive a mode change.** `src/mt/pool.rs` holds
   threads that block on a channel receive and are spawned one at a time, on
   demand, at the first dispatch that needs them — a decoder built with
   sixteen threads that never dispatches creates none. A mode change changes a
   number; it does not touch the pool.
   (`tests/adaptive.rs::threads_are_created_on_first_dispatch_and_survive_a_mode_change`,
   `a_single_threaded_decoder_never_creates_a_thread`.)

6. **Memory is accounted and bounded.** `in_flight_bytes` is buffered input
   plus blocks being decoded plus blocks decoded and not yet delivered;
   dispatch is refused rather than allowed to exceed `memory_limit`, and
   `feed` takes less than it was offered for the same reason. A run too large
   to fit at all goes to the chase decoder, which streams it, rather than
   stalling. `cancel()` stops the workers and joins them before it returns.
   (`tests/adaptive.rs::a_memory_limit_bounds_what_is_in_flight`,
   `a_run_too_large_for_the_limit_is_decoded_rather_than_stalled`,
   `cancel_stops_and_joins`.)

7. **Partial-run decode while chasing.** The chase decoder is an ordinary
   `Lzma2Decoder`, which is already resumable chunk by chunk, so a run whose
   tail has not arrived still produces output. A run it has started is never
   dispatched: dispatch happens only when the cursor sits exactly on a run's
   first byte. (`tests/adaptive.rs::a_run_decodes_before_its_tail_arrives`,
   `the_threaded_path_never_claims_a_run_the_chase_started`.)

One further deviation, in the ring itself: `Lzma2DecMt_Decode` never reads
`p->mtc.codeRes`, so a worker's `SZ_ERROR_DATA` is silently swallowed and the
decode reports success. The port returns the error and does not write the
failing block's partial output. Output written before it stays valid, which is
what a caller writing blocks by offset needs.
