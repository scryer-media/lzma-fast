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

### Multi-threaded LZMA2

```bash
cargo run --release -p lzma-bench -- --threads sweep bench/fixtures/mt.7z
```

`--threads` takes a comma-separated list of thread counts, or `sweep` for
`1,2,4,8,16,all`; `all` is this machine's available parallelism. At each
count the harness interleaves three decoders in one schedule - this crate's
`Lzma2ParallelDecoder`, `7zz t -mmt=N`, and lzma-rust2's `Lzma2ReaderMt` -
and reports the median of each and the ratio ours/oracle, so a value below
1.000 means this crate is faster. lzma-rust2 is a dependency of the harness
only; the library has none.

The in-process decoders are also measured for peak allocated bytes, through a
counting global allocator that is reset at the start of each timed decode.
`7zz` is a subprocess and is not accounted for this way.

`.7z` inputs are read by the harness's own pack-stream helper, which handles
exactly the shape the fixtures have - one file, one folder, one LZMA2 coder -
and takes the dictionary property byte from the archive's coder properties.
It is deliberately not a 7z reader: 7z is out of scope for this crate.

The two gigabyte fixtures are the two cases that matter. `mt.7z` was written
with `-mmt=on`, so it has many independently decodable runs and should scale.
`st.7z` was written with `-mmt=1`, so it has exactly one run: the parallel
decoder has to fall back to decoding it single-threaded, streaming, with no
penalty against `7zz t -mmt=1`.

## Differential correctness

`cargo test -p lzma-fast` decodes every fixture it can find under
`bench/fixtures` and small committed vectors under `crates/lzma-fast/tests`,
and compares the bytes with `xz -dc`. Fuzzing (`cargo fuzz`) targets the
decoder with arbitrary bytes and must never panic or read out of bounds.

The `decode_lzma2_mt` fuzz target is stronger than that: it decodes the same
arbitrary bytes with the single-threaded, parallel and adaptive decoders and
requires them to agree - the same output for a stream that ends at an end
marker, and a refusal from all three for one that does not.
