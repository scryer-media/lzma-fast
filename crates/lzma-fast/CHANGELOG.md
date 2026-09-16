# Changelog

## 0.2.0 (unreleased)

- Multi-threaded LZMA2 decoding behind the `std` feature, ported from
  `C/Lzma2DecMt.c` and the generic `C/MtDec.c` ring of worker threads:
  `Lzma2ParallelDecoder`, `Lzma2ParallelReader`, `Lzma2MtOptions` and
  `mt_memory_estimate`. A stream with no dictionary resets, or a run larger
  than the block budget, falls back to the single-threaded decoder for that
  region without buffering it.
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
- Optional, off-by-default support for what a container reader around LZMA
  needs: `crc` gives CRC-32 and CRC-64/XZ from `crc-fast`, and `crypto` gives
  SHA-256, unpadded AES-256-CBC and the 7z key derivation from `7zAes.c`. The
  crypto backend is RustCrypto (`sha2`, `aes`, `cbc`) by default; the `aws-lc`
  feature adds an `aws-lc-rs` one and takes precedence when both are enabled.
  The features are additive, and with both backends compiled a test requires
  them to agree.
- No dependencies in the decoder itself, no C and no build script: the assembly is `core::arch`
  inline assembly in the crate itself. Decode only.
