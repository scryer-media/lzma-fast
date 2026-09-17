# lzma-fast

[![ci](https://github.com/scryer-media/lzma-fast/actions/workflows/ci.yml/badge.svg)](https://github.com/scryer-media/lzma-fast/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/lzma-fast.svg)](https://crates.io/crates/lzma-fast)
[![docs.rs](https://docs.rs/lzma-fast/badge.svg)](https://docs.rs/lzma-fast)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/scryer-media/lzma-fast/badge)](https://securityscorecards.dev/viewer/?uri=github.com/scryer-media/lzma-fast)

Fast LZMA and LZMA2 decoding in pure Rust, ported line by line from the
7-Zip reference decoder so that its speed comes along with it.

This is a decode-only library. It decodes raw LZMA1, raw LZMA2 and the `.xz`
container; it does not encode, it does not parse `.7z`, and it is not
affiliated with 7-Zip, the LZMA SDK, XZ Utils or the Tukaani project.

## Layout

| Path | What |
| --- | --- |
| [`crates/lzma-fast`](crates/lzma-fast) | The library crate (published to crates.io). |
| [`tools/lzma-bench`](tools/lzma-bench) | Decode-throughput harness used for the acceptance gate. Not published. |
| [`docs/porting.md`](docs/porting.md) | Port rules, C-to-Rust file map, acceptance gate. |
| [`docs/benchmarking.md`](docs/benchmarking.md) | Fixtures, oracles and how to reproduce a measurement. |
| [`docs/security.md`](docs/security.md) | Every limit the container layer enforces, and what each one stops. |
| [`docs/publishing.md`](docs/publishing.md) | Release checklist. |
| [`scripts/make-fixtures.sh`](scripts/make-fixtures.sh) | Generates the benchmark fixtures locally. They are never committed. |

## Features

| Feature | Default | What it adds |
| --- | --- | --- |
| `std` | yes | The `Read` adapters and the threaded decoders. Without it the crate is `no_std` + `alloc`. |
| `asm` | yes | The hand-written decode loops ported from the LZMA SDK's `Asm/` tree, on aarch64 and x86\_64. Inert elsewhere: every other target runs the portable Rust port of the C loop. |
| `crc` | yes | CRC-32 and CRC-64/XZ from `crc-fast`, over the carry-less multiply units. |
| `crypto` | yes | SHA-256 (xz check type 10) over AWS-LC. |
| `native-crypto` | no | The same API over RustCrypto's `sha2`, for a build that wants no C toolchain. |
| `xz` | yes | The `.xz` container: `xz::XzReader`, the filters, the checks and the index. Implies `std` and `crc`. |

```rust,no_run
use std::fs::File;
use std::io::Read;
use lzma_fast::xz::XzReader;

# fn main() -> std::io::Result<()> {
let mut out = Vec::new();
XzReader::new(File::open("archive.tar.xz")?)
    .with_memory_limit(128 << 20)
    .read_to_end(&mut out)?;
# Ok(())
# }
```

A file that is seekable decodes block-parallel with `xz::XzParallelReader`,
and one that is still arriving decodes with `xz::XzAdaptiveDecoder`: input is
fed as it lands, output is drained as `(offset, bytes)`, and each block is
either handed whole to a worker or chased on the caller's thread depending on
whether all of it has arrived.

### wasm

The crate builds for `wasm32-unknown-unknown` with `--no-default-features` and
with `--no-default-features --features std,asm,crc,native-crypto,xz`, and CI
checks both. That is the whole of the decoder, the `.xz` container, the
filters and the checks; `asm` is accepted and inert there (wasm gets the
portable loop), and SHA-256 has to come from `native-crypto`, because the
default `crypto` feature builds AWS-LC's C, which wasm has no toolchain for.
The threaded decoders need real threads and are not part of that set.

## Goal

Single-threaded decode within 3% of `7zz -mmt=1` on the same input, on the
same machine, with no `unsafe`-free-at-any-cost compromises and no C. The
multi-threaded LZMA2 decoder comes after that gate is met.

## Provenance and license

The decoder is derived from `C/LzmaDec.c` and `C/Lzma2Dec.c` in the
[LZMA SDK](https://www.7-zip.org/sdk.html) by Igor Pavlov, which are in the
public domain. Copying their structure, constants and comments is therefore
deliberate and permitted. Code under 7-Zip's `CPP/` tree is LGPL and is not
used here.

This repository is licensed GPL-3.0-or-later. See [LICENSE](LICENSE).

## Contributing

See [CONTRIBUTING.md](.github/CONTRIBUTING.md) and [AGENTS.md](AGENTS.md).
Security reports go through [SECURITY.md](SECURITY.md).
