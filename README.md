# lzma-fast

[![ci](https://github.com/scryer-media/lzma-fast/actions/workflows/ci.yml/badge.svg)](https://github.com/scryer-media/lzma-fast/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/lzma-fast.svg)](https://crates.io/crates/lzma-fast)
[![docs.rs](https://docs.rs/lzma-fast/badge.svg)](https://docs.rs/lzma-fast)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/scryer-media/lzma-fast/badge)](https://securityscorecards.dev/viewer/?uri=github.com/scryer-media/lzma-fast)

Fast LZMA and LZMA2 decoding in pure Rust, ported line by line from the
7-Zip reference decoder so that its speed comes along with it.

This is a decode-only library. It does not encode, it does not parse `.7z` or
`.xz` containers, and it is not affiliated with 7-Zip, the LZMA SDK, XZ Utils
or the Tukaani project.

## Layout

| Path | What |
| --- | --- |
| [`crates/lzma-fast`](crates/lzma-fast) | The library crate (published to crates.io). |
| [`tools/lzma-bench`](tools/lzma-bench) | Decode-throughput harness used for the acceptance gate. Not published. |
| [`docs/porting.md`](docs/porting.md) | Port rules, C-to-Rust file map, acceptance gate. |
| [`docs/benchmarking.md`](docs/benchmarking.md) | Fixtures, oracles and how to reproduce a measurement. |
| [`docs/publishing.md`](docs/publishing.md) | Release checklist. |
| [`scripts/make-fixtures.sh`](scripts/make-fixtures.sh) | Generates the benchmark fixtures locally. They are never committed. |

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
