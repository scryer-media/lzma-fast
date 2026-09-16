# lzma-fast

[![crates.io](https://img.shields.io/crates/v/lzma-fast.svg)](https://crates.io/crates/lzma-fast)
[![docs.rs](https://docs.rs/lzma-fast/badge.svg)](https://docs.rs/lzma-fast)

LZMA and LZMA2 decoding in Rust, ported from the 7-Zip reference decoder for
its speed, including its hand-written `aarch64` and `x86_64` decode loops. No
C bindings, no build script, no encoder.

```toml
[dependencies]
lzma-fast = "0.2"
```

## Why another LZMA crate

Every pure-Rust LZMA decoder descends from the Tukaani XZ-for-Java design: a
readable object-oriented state machine that pays for per-bit method calls,
bounds checks and struct-resident coder state. 7-Zip's own decoder is a
different shape, one large loop with everything in registers and limits
checked once per symbol, and it decodes the same streams 1.3x to 1.75x faster
on a single core. This crate ports that shape rather than that lineage.

## Status

The decoder is real: LZMA1 and LZMA2, decode only, ported function by function
from the reference decoder. It is byte-identical to
`xz -dc` on the repository's fixtures (256 MiB LZMA1, 1 GiB LZMA1, 256 MiB
LZMA2) and on the committed vectors, including non-default `lc`/`lp`/`pb`, and
it is fuzzed for panics and out-of-bounds reads.

On `aarch64` and `x86_64` the inner loop is 7-Zip's own assembly, translated
into Rust inline assembly and selected by the default `asm` feature; turning
that feature off (`--no-default-features --features std`) falls back to the
portable Rust port of the C loop, which is what every other target uses. Both
paths are held to the same tests, and a differential test decodes every vector
with each and compares.

LZMA2 also decodes on several threads, behind the `std` feature: a port of
7-Zip's `Lzma2DecMt.c` over the `MtDec.c` ring of workers, cutting the stream
at the dictionary resets that make a run independently decodable. A stream
with no resets — what `7zz -mmt=1` produces — falls back to the
single-threaded decoder rather than buffering. Alongside it,
`Lzma2AdaptiveDecoder` decodes a stream that is still arriving: input is fed
rather than read, output is polled, and the thread count can be changed
mid-stream at run boundaries. See the "Adaptive use" section of
[docs/porting.md](../../docs/porting.md).

Either threaded decoder will also checksum its own output, in the worker that
produced it rather than on the thread draining it — `Checksum::Crc32`,
`Crc64Xz` or `Sha256`, with a `ChecksumPlan` carrying the absolute offsets
where the consumer's own boundaries (a 7z folder's sub-streams, say) fall.
Each worker emits one CRC per piece of its block between those offsets, in one
pass, and `crc::CrcFolder` folds the pieces into any range the consumer asks
about without re-reading a byte. This is not a convenience: the ring's write
callback is its one serialised section, so a CRC computed by the consumer as
it receives the output costs the decode both its own time and the queueing it
induces on every other worker — measured at 16% of an eight-thread decode.
SHA-256 cannot be folded, so it is offered per whole block only, which is the
unit an xz stream checks.

Throughput work against the acceptance gate (within 3% of `7zz t -mmt=1` on
the same file and machine) is tracked in [docs/perf-log.md](../../docs/perf-log.md);
see [docs/porting.md](../../docs/porting.md) for the plan.

## Features

| feature | default | what it adds |
| --- | --- | --- |
| `std` | yes | the `std::io::Read` adapters and `std::error::Error` |
| `asm` | yes | 7-Zip's own decode loop on `aarch64` and `x86_64` |
| `crc` | yes | CRC-32 and CRC-64/XZ, from `crc-fast`, their `CrcFolder`, and worker-side checksums in the threaded decoders |
| `crypto` | yes | SHA-256, xz check type 10, from `aws-lc-rs` |
| `native-crypto` | no | the same SHA-256 API over RustCrypto's `sha2`, taking precedence over `crypto` |

This crate is LZMA, LZMA2 and the xz container, and nothing else: 7z archives
are handled by a fork of `sevenz-rust2` that depends on it.

The decoder itself has no dependencies under any combination of these; `crc`
and `crypto` exist for the xz layer described in
[docs/porting.md](../../docs/porting.md), and nothing in `src/lzma/` or
`src/lzma2/` can reach them. `--no-default-features` builds as `no_std` +
`alloc` with none of them.

`crc` and `crypto` are on by default because every xz stream carries a check,
and a check is one of CRC-32, CRC-64/XZ or SHA-256. `crypto` builds AWS-LC,
which needs a C toolchain and CMake; a build that wants neither takes

```toml
lzma-fast = { version = "0.2", default-features = false, features = ["std", "asm", "crc", "native-crypto"] }
```

which is pure Rust and works wherever the decoder does. `native-crypto` wins
over `crypto` when both are enabled, so turning it on is an opt-out that
another crate in the graph cannot undo; with both compiled, a test requires
the two backends to produce the same digests.

## Provenance and license

The decoder is derived from `C/LzmaDec.c` and `C/Lzma2Dec.c` in the
[LZMA SDK](https://www.7-zip.org/sdk.html) by Igor Pavlov, which are in the
public domain. This crate's source is licensed GPL-3.0-or-later; see
[LICENSE](LICENSE).
