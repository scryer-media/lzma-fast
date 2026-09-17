# Benchmark fixtures

Generated locally by `cargo xtask fixtures`; every file in this directory
except this README is gitignored. Expected contents after generation:

| File | Size | How |
| --- | --- | --- |
| `payload.bin` | 1 GiB | synthetic, compresses to ~0.88 |
| `p256.bin` | 256 MiB | first 256 MiB of payload.bin |
| `p256.bin.lzma` | ~224 MiB | `xz -T1 -5 --format=lzma` (LZMA1) |
| `payload.bin.lzma` | ~897 MiB | same, on payload.bin |
| `p256.bin.xz` | ~224 MiB | `xz -T1 -5` (LZMA2 in xz container) |
| `st.7z` | ~897 MiB | `7zz a -mx5 -mmt=1` (single LZMA2 stream) |
| `mt.7z` | ~897 MiB | `7zz a -mx5` (LZMA2 with multi-threaded chunking) |
