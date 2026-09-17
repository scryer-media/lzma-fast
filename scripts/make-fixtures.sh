#!/usr/bin/env bash
# Generates the benchmark fixtures under bench/fixtures. Deterministic payload
# (seeded), so numbers are comparable across machines given the same tools.
# Requires: python3, xz (XZ Utils), 7zz (7-Zip). Needs ~4.5 GiB free.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
out="$here/bench/fixtures"
mkdir -p "$out"
cd "$out"

if [ ! -f payload.bin ]; then
  echo "generating payload.bin (1 GiB, semi-compressible, seed 7)"
  python3 - <<'PY'
import os, random
random.seed(7)
words = [os.urandom(random.randint(3, 9)).hex() for _ in range(4000)]
with open('payload.bin', 'wb') as f:
    for _ in range(1024):
        chunk = bytearray()
        while len(chunk) < 1 << 20:
            if random.random() < 0.7:
                chunk += random.choice(words).encode() + b' '
            else:
                chunk += os.urandom(random.randint(16, 256))
        f.write(chunk[:1 << 20])
PY
fi
# NOTE: os.urandom makes the random-byte runs non-deterministic; the token
# stream and the compression ratio are stable, byte identity across machines
# is not. Compare timings on one machine, not fixture hashes across machines.

[ -f p256.bin ] || head -c 268435456 payload.bin > p256.bin

# LZMA1 (LZMA-alone container, 13-byte header) at xz preset 5, single thread.
[ -f p256.bin.lzma ]    || xz -T1 -5 --format=lzma -c p256.bin    > p256.bin.lzma
[ -f payload.bin.lzma ] || xz -T1 -5 --format=lzma -c payload.bin > payload.bin.lzma

# LZMA2 inside xz, preset 5, single stream.
[ -f p256.bin.xz ] || xz -T1 -5 -c p256.bin > p256.bin.xz

# LZMA2 inside 7z: st = one stream, mt = 7-Zip's multi-threaded chunking.
[ -f st.7z ] || 7zz a -bso0 -bsp0 -mx=5 -m0=lzma2 -mmt=1  st.7z payload.bin
[ -f mt.7z ] || 7zz a -bso0 -bsp0 -mx=5 -m0=lzma2 -mmt=on mt.7z payload.bin


# --- the .xz container lane -------------------------------------------------
# These are the fixtures the `--xz` mode of lzma-bench times: whole files with
# real block layouts, filters and checks, rather than one long LZMA2 stream.
# They are gitignored like the rest of bench/fixtures; nothing here is
# committed.

# Multi-block, which is the only shape a parallel decode can help with. `-T8`
# picks the block size from the thread count; the 16 MiB one fixes it, so the
# two rows say what block size costs independently of how many there are.
[ -f p256.t8.xz ]     || xz -T8 -5 -c p256.bin > p256.t8.xz
[ -f p256.b16.xz ]    || xz -T8 -5 --block-size=16MiB -c p256.bin > p256.b16.xz
[ -f payload.t8.xz ]  || xz -T8 -5 -c payload.bin > payload.t8.xz

# The filters, so the converter pipeline is timed and not just LZMA2. The x86
# filter wants something with x86 code in it; the local `xz` binary is the one
# executable every machine running this script has.
[ -f bcj-x86.xz ] || xz -T1 -5 --x86 --lzma2=preset=5 -c "$(command -v xz)" > bcj-x86.xz
[ -f delta.xz ]   || xz -T1 -5 --delta=dist=4 --lzma2=preset=5 -c p256.bin > delta.xz

# The checks. CRC-64 is the default and is already covered by the rows above.
[ -f p256.sha256.xz ] || xz -T1 -5 --check=sha256 -c p256.bin > p256.sha256.xz
[ -f p256.crc32.xz ]  || xz -T1 -5 --check=crc32  -c p256.bin > p256.crc32.xz

# Several streams in one file, which is what `xz -d` accepts by default and
# what weaver's decoder is configured for.
[ -f multi.xz ] || cat p256.xz p256.xz p256.xz > multi.xz

# And a real-world shape: a tarball, which is what an .xz usually is.
if [ ! -f tree.tar.xz ]; then
  tar cf - -C "$here" crates tools docs scripts 2>/dev/null | xz -T8 -5 > tree.tar.xz
fi

ls -l "$out"
