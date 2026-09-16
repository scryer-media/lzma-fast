# Changelog

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
