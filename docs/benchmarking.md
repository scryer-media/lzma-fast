# Benchmarking

## Fixtures

Run `scripts/make-fixtures.sh` once. It writes to `bench/fixtures/` (ignored).
See `bench/fixtures/README.md` for the list.

## Oracles

| Oracle | Command | What it measures |
| --- | --- | --- |
| 7-Zip shipped decoder (asm loop) | `7zz t -mmt=1 st.7z` / `7zz t -mmt=1 p256.bin.lzma` | The acceptance target. `t` decodes and CRCs without writing. |
| 7-Zip C decoder (no asm) | `~/dev/supporting-codebases/7zip/C/Util/Lzma/_o/7lzma d in.lzma /dev/null` | C parity checkpoint. Built from `C/Util/Lzma` with the default makefile (`make -f makefile.gcc`). The harness looks there first and falls back to whatever `7lzma` is on `PATH`, which is how a bench box with its own build is reached. |
| XZ Utils | `xz -dc -T1 in.xz > /dev/null` / `xz -dc --format=lzma in.lzma > /dev/null` | Tukaani C decoder; also the correctness reference for output bytes. |

Reference numbers on Apple M5 Max, 7-Zip 26.01, XZ Utils 5.8 (2026-09-15):

| Input | `7zz -mmt=1` | `xz -T1` | lzma-rust2 0.20.1 | sevenz-rust2 0.22.2 |
| --- | --- | --- | --- | --- |
| `p256.bin.xz` (256 MiB) | 3.9 s | 6.0 s | 6.6 s | – |
| `st.7z` (1 GiB) | 17.9 s | – | – | 25.1 s |
| `mt.7z` (1 GiB) | 18.9 s | – | – | 25.0 s |

## Method

```bash
cargo run --release -p lzma-bench -- bench/fixtures/p256.bin.lzma
```

- Idle machine, mains power, three runs, report the median.
- Compare against the oracle measured in the same session; do not reuse
  numbers from another day.
- The bench tool decodes into a discard sink and checks the output CRC/length
  against the oracle's so speed never comes from skipped work.
- For profiling use `cargo build --profile profiling` (symbols kept) and
  `samply` / `perf`.

## Differential correctness

`cargo test -p lzma-fast` decodes every fixture it can find under
`bench/fixtures` and small committed vectors under `crates/lzma-fast/tests`,
and compares the bytes with `xz -dc`. Fuzzing (`cargo fuzz`) targets the
decoder with arbitrary bytes and must never panic or read out of bounds.
