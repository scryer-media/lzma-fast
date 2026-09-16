# lzma-fast

[![crates.io](https://img.shields.io/crates/v/lzma-fast.svg)](https://crates.io/crates/lzma-fast)
[![docs.rs](https://docs.rs/lzma-fast/badge.svg)](https://docs.rs/lzma-fast)

LZMA and LZMA2 decoding in Rust, ported from the 7-Zip reference decoder for
its speed. No C bindings, no assembly, no encoder.

```toml
[dependencies]
lzma-fast = "0.1"
```

## Why another LZMA crate

Every pure-Rust LZMA decoder descends from the Tukaani XZ-for-Java design: a
readable object-oriented state machine that pays for per-bit method calls,
bounds checks and struct-resident coder state. 7-Zip's own decoder is a
different shape, one large loop with everything in registers and limits
checked once per symbol, and it decodes the same streams 1.3x to 1.75x faster
on a single core. This crate ports that shape rather than that lineage.

## Status

Scaffold. The decoder is being ported; see
[docs/porting.md](../../docs/porting.md) for the plan and the acceptance gate
(within 3% of `7zz` single-threaded on the repository's fixtures).

## Provenance and license

The decoder is derived from `C/LzmaDec.c` and `C/Lzma2Dec.c` in the
[LZMA SDK](https://www.7-zip.org/sdk.html) by Igor Pavlov, which are in the
public domain. This crate's source is licensed GPL-3.0-or-later; see
[LICENSE](LICENSE).
