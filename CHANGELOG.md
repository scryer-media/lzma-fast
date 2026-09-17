# Changelog

## 0.3.3 - 2026-09-17

0.3.2 was tagged and never reached crates.io; this carries its change too.

- An `.xz` block that declares no uncompressed size and runs past
  `max_unpack_bytes` (or the block cap) now fails with `TooMuchOutput`. It was
  reported as corrupt LZMA data: once the allowance was used up, LZMA2 was
  asked to read the end marker, failed on the chunk that followed instead, and
  that failure was returned as it stood. A single-threaded `xz` writes such
  blocks, so a caller hitting its own limit was told its file was broken.
- The committed test vectors are now marked binary, which fixes the Windows
  test lane. Git decides text from bytes by looking for a zero byte, and one
  small vector's uncompressed source file happens to have none, so a Windows
  checkout rewrote its three line endings and the file arrived three bytes
  longer than the stream it was compressed from. Every test that compared the
  two failed, on the assembly loop and on the portable one alike. The library
  was never affected - it decodes those streams correctly on every platform -
  but the Windows lane had therefore never been green, and nothing in the
  release gate ran the tests anywhere but Linux. The release workflow now
  runs the Windows and macOS lanes as well, and publishing waits for them.

## 0.3.2 - 2026-09-17

- The `aarch64` decode loop's assembly file now purges every macro it defines.
  Its macros are named after x86 mnemonics, and one of them, `shl`, is also a
  NEON instruction. An assembler macro outlives the file that defined it, so
  under LTO it shadowed `shl` in other crates' assembly and their build failed
  with "too many positional arguments". Seen on `aarch64-unknown-linux`; the
  decoder itself is unchanged.

## 0.3.1 - 2026-09-17

The crate is published as `lzma-turbo`. It was briefly on crates.io as
`lzma-fast`, with the same contents as 0.3.0; that name is
withdrawn. Only the crate and library names changed: replace `lzma_fast` with
`lzma_turbo` in paths and `lzma-fast` with `lzma-turbo` in manifests.

## 0.3.0 - 2026-09-17

The `.xz` container, behind the new default `xz` feature. 0.2.0's entry is
kept below as it stands: that work is unreleased, but the container is a
second public surface of its own size - a module, a reader, a filter set and a
dozen error variants - and versioning it separately keeps the two stories
apart for anyone reading the history.

- `xz::XzReader`: a `Read` adapter over an `.xz` file. Reads every stream in
  the file by default, as `xz -d` does, and verifies the stream header and
  footer, every block header's CRC-32 *before* believing any field in it, the
  declared compressed and uncompressed sizes, each block's check, the block
  and stream padding, and the index - against a running fold of the blocks
  rather than a list of them, so a stream of a million blocks costs the same
  state as a stream of one. `with_memory_limit`, `with_checks`,
  `allow_unverifiable` and `single_stream` on the reader; everything else on
  `xz::XzOptions`.
- Filters: LZMA2, delta, and the eight BCJ converters (x86, ARM, ARM-Thumb,
  ARM64, PowerPC, SPARC, IA-64, RISC-V), each with the optional four-byte
  start-offset property, in chains of up to four in the order the format
  allows. Ported from `C/Bra.c`, `C/Bra86.c`, `C/BraIA64.c` and `C/Delta.c`,
  and public as `xz::bcj` and `xz::delta` so that other container code in this
  workspace can use them directly.
- Checks: none, CRC-32, CRC-64/XZ and SHA-256 (the last behind `crypto` or
  `native-crypto`). A stream whose check this build cannot compute is an error
  unless the caller opts in with `allow_unverifiable`, so unchecked bytes are
  never returned silently. The check is computed by whoever decoded the block,
  through the same `ChecksumPlan` machinery the LZMA2 workers use, and the
  caller's own split points are computed in the same pass.
- `xz::XzParallelReader`: block-parallel decoding of a seekable `.xz` file,
  ported in spirit from `C/XzDecMt.c`. The index of every stream is read
  first, so every block's offset and both its sizes are known before anything
  is decoded: blocks are scheduled directly, a worker's output buffer is also
  its dictionary, and the thread count is *degraded to fit* the caller's
  memory limit rather than the decode failing halfway through.
  `memory_estimate`, `threads`, `block_count` and `uncompressed_size` report
  what it will cost and what it will produce. Concatenated streams and stream
  padding are handled by walking the file's footers backwards. The source has
  to be `Read + Seek` but not `Send`; it is only ever read on the caller's
  thread.
- `xz::XzAdaptiveDecoder`: decoding an `.xz` file that is still arriving.
  Input is fed rather than read and output is drained as `(offset, bytes)`, so
  a caller chasing a download can write what it gets by position. Each block
  decides its own mode: a block whose header declares both its sizes and whose
  bytes have all arrived goes to a worker whole, and everything else - the
  tail being written, and any block whose header declares no compressed size -
  is chased on the caller's thread as it arrives. Blocks are consumed in file
  order and the chase runs only when no worker is outstanding, so output is
  always in order. `set_threads` takes effect at the next block, and
  `in_flight_bytes` reports what is held.
- `xz::XzAdaptiveDecoder::drain_upto(limit, sink)`: as `drain`, but stops once
  the sink has been handed `limit` bytes and keeps the rest of the block it was
  in the middle of for the next call. A caller implementing `Read` over the
  decoder can hand it the caller's own buffer instead of spilling whole blocks
  into one of its own, which makes its memory a function of what is in flight
  rather than of what has been fed.
- `xz::stream_table` and `xz::block_table`: every stream, and every block,
  located from the indexes of a seekable file without decoding anything.
  `block_table` is the whole file's block list in file order with output
  offsets relative to the file, which is what a caller deciding whether to
  widen a decode wants to see before it commits. `XzParallelReader` now uses
  `stream_table` rather than its own copy of the walk.
- `xz::probe`, `xz::single_stream_block_count` and
  `xz::is_single_stream_multi_block`: structural gates over the footer and
  index that decode nothing, for a caller choosing between a sequential and a
  parallel decode.
- `xz::XzIndex` and `xz::read_stream_index_ending_at`: the index of a stream,
  parsed from its footer, with per-block file offsets and sizes.
- `xz::XzError` carries the stream, the block and the file offset of every
  failure, and converts to `std::io::Error`.
- Every allocation the container makes is bounded before it is made, and a
  block's dictionary is clamped to the block's own declared uncompressed size,
  which is usually far below the dictionary the stream declares. The limits
  are listed in `docs/security.md`.

### LZMA2, asked for by the `sevenz-turbo` fork

- `Lzma2AdaptiveDecoder` no longer decodes on the calling thread while a worker
  is outstanding, and `set_chase(false)` turns the chase off for a caller whose
  input is already on disk. The chase decoder serialises the whole decoder
  while it holds the cursor - no worker may claim a run - and that is only the
  right trade when there is nothing else in flight, which is the
  arriving-stream case it was built for. For a stream already on disk it cost
  the fork a measured 1.50x against the same decoder's own parallel path, and
  measuring it here showed why the no-worker rule alone is not enough: fed in
  pieces smaller than a run, the chase takes every run before a worker can see
  it, and the decode never threads at all (21.7 s at one thread, 23.9 s at
  eight). With chasing off the decoder waits for the rest of the run instead,
  and the curve comes back. Off is advisory, not absolute: a run too large for
  the memory limit, a run the chase has already started, a single-threaded
  decoder, and everything after `end_of_input` are still decoded inline,
  because nothing else would decode them.
- `Lzma2AdaptiveDecoder::drain_upto(limit, sink)`, as on `XzAdaptiveDecoder`
  above and for the same reason.
- `lzma_turbo::run_boundaries(source, dict_prop)`: the runs of an LZMA2 stream
  in a `Read + Seek` source, found by seeking past chunk payloads rather than
  reading them, with the source's position restored. `Lzma2RunScanner` answers
  this for bytes as they arrive; this answers it for bytes already on disk.
  `Lzma2RunScanner::payload_remaining` and `skip_payload` are the two methods
  that make skipping possible, and are public for callers driving the scanner
  over their own source.
- `Lzma2ParallelReader`'s `Read + Send + 'static` bound is documented rather
  than removed: the reader is moved onto a thread that outlives the call, so a
  scoped thread cannot serve it. `docs/porting.md` has the table of which
  driver to use for a borrowed source.

### Fixed

- A block whose decode was interrupted by an earlier block's error is no
  longer decoded at all. Its `pre_code` was skipped - which is what lends the
  coder's buffer to the decoder as its dictionary - while the code loop ran
  anyway, so the block decoded into an empty dictionary: a panic in the
  portable loop and a store through a dangling pointer in the assembly one.
  Seen as a segfault on macOS and as `STATUS_ACCESS_VIOLATION` on
  windows-msvc, on corrupt input only, and reproduced in a no-assembly build
  on both.
- A bounded `drain_upto` no longer reports a truncated LZMA2 stream as
  finished. The chase decoder can stop mid-chunk when the caller's budget runs
  out; with the end marker as the next input byte, the input cursor reached
  the end of the stream with a chunk still owing output, and that was taken
  for a clean end. The stream now ends only where the chase decoder is between
  chunks. Found by fuzzing the new drain budget.
- `Lzma2AdaptiveDecoder` fed far ahead of a stream of small runs no longer
  spends its time moving its own input. The consumed front of the input buffer
  was dropped after every dispatched run, which moves everything behind it:
  with a gigabyte fed and 1 MiB runs - what `7zz -mx1` writes for data that
  does not compress - that was most of a gigabyte moved a thousand times. It is
  now dropped once it is half the buffer, or when `feed` needs the room. A
  1 GiB archive of that shape through the `sevenz-turbo` reader at 18 threads:
  4.6 s before, 1.3 s after, against 1.1 s for `7zz t`.

Packaging: the crate is the repository root (it was `crates/lzma-turbo`), and
its archive carries the library, the README, the changelog and the license
only. The tests share a helper with the benchmark tool and read repository
fixtures, so they run from a checkout.

## 0.2.0 (unreleased)

- Multi-threaded LZMA2 decoding behind the `std` feature, ported from
  `C/Lzma2DecMt.c` and the generic `C/MtDec.c` ring of worker threads:
  `Lzma2ParallelDecoder`, `Lzma2ParallelReader`, `Lzma2MtOptions` and
  `mt_memory_estimate`. A stream with no dictionary resets, or a run larger
  than the block budget, falls back to the single-threaded decoder for that
  region without buffering it.
- `Lzma2AdaptiveDecoder`: LZMA2 decoding for a stream that is still arriving.
  Input is fed rather than read and never blocks, output is polled as
  `(offset, bytes)` blocks in order or as decoded, the thread count can be
  changed mid-stream and takes effect at the next run boundary, memory in
  flight is accounted and bounded, and the decode can be cancelled. Worker
  threads are created at the first dispatch and parked, not torn down, across
  a mode change.
- Worker-side checksums for both threaded decoders: `Checksum::{None, Crc32,
  Crc64Xz, Sha256}` with `ChecksumPlan`, `Lzma2ParallelDecoder::decode_checksummed`,
  `Lzma2ParallelReader::with_checksums` / `take_checks` / `take_segments` and
  `Lzma2AdaptiveDecoder::set_checksum` / `take_checks`. Each worker checksums
  the block it produced before it queues for the write token, so the work is
  parallel; a checksum computed by the consumer as it drains runs inside the
  ring's one serialised section instead, which measured at 16% of an
  eight-thread decode. The caller passes the absolute unpacked offsets where
  its own boundaries fall (a 7z folder's sub-streams, say) and gets one CRC
  per piece between them, in a single pass. SHA-256 is per whole block and
  ignores split points, because it cannot be folded. Behind `crc`, with
  SHA-256 additionally behind `crypto` or `native-crypto`; opting out of all
  of it leaves the decode paths exactly as they were.
- `crc::CrcFolder`, `crc::Foldable`, `crc::crc32_combine` and
  `crc::crc64_xz_combine`: fold the checksums of pieces of a stream, pushed in
  any order, into the checksum of any contiguous range they tile, without
  re-reading a byte. Available without `std`.
- Measured against 7-Zip on a gigabyte: the parallel decoder is within a few
  per cent of `7zz t -mmt=N` across the whole thread curve on both aarch64
  macOS and x86_64 Linux, and 1.6x to 12x faster than lzma-rust2's
  `Lzma2ReaderMt` at every thread count. See `docs/perf-log.md`.
- `Lzma2RunScanner` and `Lzma2Run`: incremental, public discovery of the
  independently decodable runs in an LZMA2 stream, costing O(chunks) and no
  decoding.
- `Error::CorruptRun` locates corruption by run index and output offset, and
  `Error::Cancelled` reports a cancelled decode.
- Removed `crypto::Aes256Cbc` and `crypto::sevenz_key`, and with them the
  `aes` and `cbc` dependencies. The crate is LZMA, LZMA2 and xz; 7z archives
  — the header, folders, coder graphs, BCJ and delta filters, AES-256 and the
  `7zAes.c` key derivation — are a separate crate's job, a fork of
  `sevenz-rust2` that depends on this one. `crypto` now provides SHA-256
  alone, which is what an xz stream with check type 10 needs.
- Crypto backends swapped round, and both checks turned on by default:
  `crypto` (now default) is SHA-256 over `aws-lc-rs`, and the new
  `native-crypto` is the RustCrypto `sha2` one, taking precedence when both
  are enabled so that opting out of the C build cannot be undone by another
  crate in the graph. The `aws-lc` feature name is gone. `crc` is also default
  now, because every xz stream carries a check and two of the three check
  types are CRC-32 and CRC-64/XZ. `--no-default-features` still builds as
  `no_std` + `alloc` with none of it, and nothing in `src/lzma/` or
  `src/lzma2/` can reach any of it either way.
- `Lzma2Dec_Parse` is ported as `lzma2::parse`, and the LZMA2 chunk-header
  state machine it shares with the decoder is factored out into `lzma2::frame`
  so the parser and the decoder cannot drift apart.

## 0.1.0 (unreleased)

- LZMA1 and LZMA2 decoding, ported function by function from Igor Pavlov's
  `C/LzmaDec.c` and `C/Lzma2Dec.c` (LZMA SDK 26.03, public domain): the C fast
  loop, the `LzmaDec_TryDummy` careful path, `LzmaDec_WriteRem`,
  `LzmaDec_DecodeToDic` / `DecodeToBuf`, `LzmaProps_Decode`, and the LZMA2
  chunk framing with its prop, state and dictionary resets.
- `LzmaDecoder`, `Lzma2Decoder`, `LzmaProps`, `LzmaAloneHeader`, and the
  `LzmaReader` / `Lzma2Reader` `std::io::Read` adapters behind the default
  `std` feature. The crate also builds `--no-default-features` as `no_std` +
  `alloc`.
- The `asm` feature (on by default): the hand-written decode loops from the
  SDK's `Asm/arm64/LzmaDecOpt.S` and `Asm/x86/LzmaDecOpt.asm`, translated line
  by line into Rust `naked_asm!` and used on `aarch64` and `x86_64`. Every
  other target, and `--no-default-features --features std`, get the portable
  Rust port of the C loop, which stays the differential reference for the
  assembly.
- Optional support for what a container reader around LZMA needs: `crc` gives
  CRC-32 and CRC-64/XZ from `crc-fast`, and `crypto` gives SHA-256 with a
  choice of backend. The features are additive, and with both crypto backends
  compiled a test requires them to agree. (See 0.2.0 for the backend and
  default-feature layout these ended up with.)
- No dependencies in the decoder itself, no C and no build script: the assembly is `core::arch`
  inline assembly in the crate itself. Decode only.
