# lzma-fast

[![ci](https://github.com/scryer-media/lzma-fast/actions/workflows/ci.yml/badge.svg)](https://github.com/scryer-media/lzma-fast/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/lzma-fast.svg)](https://crates.io/crates/lzma-fast)
[![docs.rs](https://docs.rs/lzma-fast/badge.svg)](https://docs.rs/lzma-fast)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/scryer-media/lzma-fast/badge)](https://securityscorecards.dev/viewer/?uri=github.com/scryer-media/lzma-fast)

LZMA and LZMA2 decoding in Rust, ported from the 7-Zip reference decoder for
its speed, including its hand-written `aarch64` and `x86_64` decode loops. No
C bindings, no build script, no encoder.

```toml
[dependencies]
lzma-fast = "0.3"
```

## Reading an `.xz` file

```rust
use std::fs::File;
use std::io::Read;
use lzma_fast::xz::XzReader;

fn main() -> std::io::Result<()> {
    let mut out = Vec::new();
    XzReader::new(File::open("archive.tar.xz")?)
        .with_memory_limit(128 << 20)
        .read_to_end(&mut out)?;
    Ok(())
}
```

A file that is seekable decodes block-parallel with `xz::XzParallelReader`,
and one that is still arriving decodes with `xz::XzAdaptiveDecoder`: input is
fed as it lands, output is drained as `(offset, bytes)`, and each block is
either handed whole to a worker or chased on the caller's thread depending on
whether all of it has arrived.

## Why another LZMA crate

Every pure-Rust LZMA decoder descends from the Tukaani XZ-for-Java design: a
readable object-oriented state machine that pays for per-bit method calls,
bounds checks and struct-resident coder state. 7-Zip's own decoder is a
different shape, one large loop with everything in registers and limits
checked once per symbol, and it decodes the same streams 1.3x to 1.75x faster
on a single core. This crate ports that shape rather than that lineage.

## Speed

Linux x86_64, Intel Arrow Lake-H, 16 threads, median of three runs. Every
row decodes the same bytes and is checked byte for byte against `xz`.

One thread:

| stream | lzma-fast | `7zz -mmt=1` | `xz -T1` |
| --- | --- | --- | --- |
| 1 GiB LZMA1 | 19.6 s | 19.6 s | 20.5 s |
| 256 MiB LZMA2 (.xz) | 4.9 s | 5.1 s | 5.1 s |

LZMA2 in parallel, a 1 GiB stream written by `7zz -mmt=on`:

| threads | lzma-fast | `7zz` | lzma-rust2 |
| --- | --- | --- | --- |
| 1 | 20.4 s | 20.5 s | 29.6 s |
| 4 | 6.5 s | 6.5 s | 29.3 s |
| 8 | 4.3 s | 3.9 s | 29.3 s |
| 16 | 4.2 s | 3.9 s | 29.0 s |

`.xz` in parallel, 256 MiB in 16 blocks:

| threads | `XzParallelReader` | `xz -T<n>` |
| --- | --- | --- |
| 1 | 5.0 s | 5.2 s |
| 4 | 1.6 s | 1.6 s |
| 16 | 0.60 s | 0.69 s |

The 8- and 16-thread LZMA2 rows are the one place `7zz` is ahead, by how
well its threads land on this machine's performance cores; pinned to those
cores the two are within 5%. The full tables, with peak memory and the macOS
and Windows rows, are in
[docs/perf-log.md](https://github.com/scryer-media/lzma-fast/blob/main/docs/perf-log.md).

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
[docs/porting.md](https://github.com/scryer-media/lzma-fast/blob/main/docs/porting.md).

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
the same file and machine) is tracked in [docs/perf-log.md](https://github.com/scryer-media/lzma-fast/blob/main/docs/perf-log.md);
see [docs/porting.md](https://github.com/scryer-media/lzma-fast/blob/main/docs/porting.md) for the plan.

## Features

| feature | default | what it adds |
| --- | --- | --- |
| `std` | yes | the `std::io::Read` adapters and `std::error::Error` |
| `asm` | yes | 7-Zip's own decode loop on `aarch64` and `x86_64` |
| `crc` | yes | CRC-32 and CRC-64/XZ, from `crc-fast`, their `CrcFolder`, and worker-side checksums in the threaded decoders |
| `crypto` | yes | SHA-256, xz check type 10, from `aws-lc-rs` |
| `xz` | yes | the `.xz` container: `xz::XzReader`, `XzParallelReader`, `XzAdaptiveDecoder`, the filters, the checks and the index; implies `std` and `crc` |
| `native-crypto` | no | the same SHA-256 API over RustCrypto's `sha2`, taking precedence over `crypto` |

This crate is LZMA, LZMA2 and the xz container, and nothing else: 7z archives
are handled by a fork of `sevenz-rust2` that depends on it.

The decoder itself has no dependencies under any combination of these; `crc`
and `crypto` exist for the xz layer described in
[docs/porting.md](https://github.com/scryer-media/lzma-fast/blob/main/docs/porting.md), and nothing in `src/lzma/` or
`src/lzma2/` can reach them. `--no-default-features` builds as `no_std` +
`alloc` with none of them.

`crc` and `crypto` are on by default because every xz stream carries a check,
and a check is one of CRC-32, CRC-64/XZ or SHA-256. `crypto` builds AWS-LC,
which needs a C toolchain and CMake; a build that wants neither takes

```toml
lzma-fast = { version = "0.3", default-features = false, features = ["std", "asm", "crc", "native-crypto"] }
```

which is pure Rust and works wherever the decoder does. `native-crypto` wins
over `crypto` when both are enabled, so turning it on is an opt-out that
another crate in the graph cannot undo; with both compiled, a test requires
the two backends to produce the same digests.

## wasm

The crate builds for `wasm32-unknown-unknown` with `--no-default-features` and
with `--no-default-features --features std,asm,crc,native-crypto,xz`, and CI
checks both. That is the whole of the decoder, the `.xz` container, the
filters and the checks; `asm` is accepted and inert there (wasm gets the
portable loop), and SHA-256 has to come from `native-crypto`, because the
default `crypto` feature builds AWS-LC's C, which wasm has no toolchain for.
The threaded decoders need real threads and are not part of that set.

## Platforms

Built and tested on macOS aarch64, Linux x86-64 and windows-msvc x86-64. The
Windows lane is built with `clang-cl` and links AWS-LC statically, both with
its assembly generated from source by NASM (`AWS_LC_SYS_PREBUILT_NASM=0`) and
through its prebuilt objects; the test suite is run there in the assembly
build and in the portable (`--no-default-features --features std,crc`) build,
and `docs/perf-log.md` carries its `.xz` numbers.

## Repository layout

| Path | What |
| --- | --- |
| [`src`](src), [`tests`](tests), [`fuzz`](fuzz) | The library crate (published to crates.io), its tests and its fuzz targets. |
| [`tools/lzma-bench`](tools/lzma-bench) | Decode-throughput harness used for the acceptance gate. Not published. |
| [`docs/porting.md`](docs/porting.md) | Port rules, C-to-Rust file map, acceptance gate. |
| [`docs/benchmarking.md`](docs/benchmarking.md) | Fixtures, oracles and how to reproduce a measurement. |
| [`docs/security.md`](docs/security.md) | Every limit the container layer enforces, and what each one stops. |
| [`docs/publishing.md`](docs/publishing.md) | Release checklist. |
| [`xtask`](xtask) | `cargo xtask`: the release checks, the fixture generator, the pre-commit hook and the asm-lab harness, in dependency-free Rust. Not published. |
| `cargo xtask fixtures` | Generates the benchmark fixtures locally. They are never committed. |

## Acknowledgements

This crate is a translation, and the people whose work it translates deserve
to be named first.

- **Igor Pavlov** designed LZMA and LZMA2 and wrote 7-Zip and the
  [LZMA SDK](https://www.7-zip.org/sdk.html). Every decoder in this crate is a
  port of his: `LzmaDec.c` and `Lzma2Dec.c`, the multi-threaded
  `Lzma2DecMt.c` over `MtDec.c`, and the `aarch64` and `x86_64` decode loops
  from the SDK's `Asm/` tree. The speed this crate is named for is the shape
  of his code, kept as faithfully as Rust allows, and he placed all of it in
  the public domain.
- **Lasse Collin** and the [Tukaani project](https://tukaani.org/xz/) wrote
  the `.xz` format specification and `xz`, which is the oracle every stream
  this crate decodes is checked against, byte for byte.
- The **XZ for Java** lineage of pure-Rust decoders, whose readability set
  the bar this crate measures itself against, and whose authors made LZMA in
  Rust normal long before this port existed.

## License

The decoder is derived from `C/LzmaDec.c`, `C/Lzma2Dec.c`, `C/Lzma2DecMt.c`,
`C/MtDec.c` and `Asm/` in the LZMA SDK by Igor Pavlov, which are in the
public domain. This crate's source is licensed GPL-3.0-or-later; see
[LICENSE](LICENSE).

## Contributing

See [CONTRIBUTING.md](.github/CONTRIBUTING.md) and [AGENTS.md](AGENTS.md).
Security reports go through [SECURITY.md](SECURITY.md).
